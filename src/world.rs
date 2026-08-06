use std::collections::{HashMap, HashSet};

use glam::{IVec3, Vec3};

use crate::perlin::PerlinNoise;

pub const CHUNK_SIZE: i32 = 16;
pub const WORLD_HEIGHT: i32 = 48;
pub const RENDER_DISTANCE: i32 = 3;
const SEA_LEVEL: i32 = 12;

/// Full daylight level stored per block.
const MAX_SKYLIGHT: u8 = 15;

/// How far a tree may spill outside the chunk that owns its trunk. Chunks scan
/// this margin into their neighbours so canopies are never cut at a border.
const TREE_MARGIN: i32 = 3;

/// Chunks generated per call to `load_chunks_around`. Sized so a frame never
/// spends more than a fraction of its budget on terrain generation.
const GENERATION_BUDGET: usize = 8;

/// One block of padding around a chunk is enough to mesh it: faces need the
/// direct neighbour, ambient occlusion needs the diagonals.
const PAD: i32 = 1;
const PADDED_SIZE: i32 = CHUNK_SIZE + 2 * PAD;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Block {
    Air,
    Grass,
    Dirt,
    Stone,
    Sand,
    Wood,
    Leaves,
    Snow,
}

impl Block {
    fn is_solid(self) -> bool {
        self != Block::Air
    }

    /// Blocks that stop skylight from travelling further down a column.
    fn blocks_light(self) -> bool {
        self != Block::Air
    }
}

/// Palette index handed to the GPU. Grass needs two entries because its top
/// face is green while its sides are earthy.
#[derive(Clone, Copy)]
#[repr(u8)]
enum Material {
    GrassTop = 0,
    GrassSide = 1,
    Dirt = 2,
    Stone = 3,
    Sand = 4,
    Wood = 5,
    Leaves = 6,
    Snow = 7,
}

fn material_for(block: Block, face: usize) -> u32 {
    let top = face == FACE_POSITIVE_Y;
    let material = match block {
        Block::Grass if top => Material::GrassTop,
        Block::Grass => Material::GrassSide,
        Block::Dirt => Material::Dirt,
        Block::Stone => Material::Stone,
        Block::Sand => Material::Sand,
        Block::Wood => Material::Wood,
        Block::Leaves => Material::Leaves,
        Block::Snow => Material::Snow,
        Block::Air => Material::Stone,
    };
    material as u32
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub struct ChunkPosition {
    pub x: i32,
    pub z: i32,
}

impl ChunkPosition {
    fn from_world(x: i32, z: i32) -> Self {
        Self {
            x: x.div_euclid(CHUNK_SIZE),
            z: z.div_euclid(CHUNK_SIZE),
        }
    }
}

struct Chunk {
    blocks: Vec<Block>,
    skylight: Vec<u8>,
    /// One past the highest non-air block, so meshing can stop early instead of
    /// scanning the full WORLD_HEIGHT of mostly empty sky.
    max_height: i32,
}

impl Chunk {
    fn generate(position: ChunkPosition, noise: &PerlinNoise, seed: u32) -> Self {
        let mut chunk = Self {
            blocks: vec![Block::Air; (CHUNK_SIZE * WORLD_HEIGHT * CHUNK_SIZE) as usize],
            skylight: vec![0; (CHUNK_SIZE * WORLD_HEIGHT * CHUNK_SIZE) as usize],
            max_height: 0,
        };

        for local_z in 0..CHUNK_SIZE {
            for local_x in 0..CHUNK_SIZE {
                let world_x = position.x * CHUNK_SIZE + local_x;
                let world_z = position.z * CHUNK_SIZE + local_z;
                let height = terrain_height(noise, world_x, world_z);

                for y in 0..=height {
                    chunk.set_raw(IVec3::new(local_x, y, local_z), surface_block(height, y));
                }
            }
        }

        chunk.plant_trees(position, noise, seed);
        chunk.refresh_max_height();
        chunk.recompute_skylight();
        chunk
    }

    /// Trees are placed from a deterministic hash of their world coordinates, so
    /// every chunk that overlaps a canopy reproduces exactly the same blocks
    /// without any cross-chunk coordination.
    fn plant_trees(&mut self, position: ChunkPosition, noise: &PerlinNoise, seed: u32) {
        let origin_x = position.x * CHUNK_SIZE;
        let origin_z = position.z * CHUNK_SIZE;

        for offset_z in -TREE_MARGIN..CHUNK_SIZE + TREE_MARGIN {
            for offset_x in -TREE_MARGIN..CHUNK_SIZE + TREE_MARGIN {
                let world_x = origin_x + offset_x;
                let world_z = origin_z + offset_z;
                let Some(trunk_height) = tree_at(seed, noise, world_x, world_z) else {
                    continue;
                };

                let ground = terrain_height(noise, world_x, world_z);
                self.carve_tree(offset_x, offset_z, ground, trunk_height);
            }
        }
    }

    /// Writes the parts of a tree that land inside this chunk. `local_x`/`local_z`
    /// may sit outside 0..CHUNK_SIZE — those blocks belong to a neighbour and are
    /// simply skipped by `set_raw`.
    fn carve_tree(&mut self, local_x: i32, local_z: i32, ground: i32, trunk_height: i32) {
        let base = ground + 1;
        let top = base + trunk_height;

        for y in base..top {
            self.set_raw(IVec3::new(local_x, y, local_z), Block::Wood);
        }

        // Two wide layers then a small cap: the classic oak silhouette.
        for (offset, radius) in [(-2_i32, 2_i32), (-1, 2), (0, 1), (1, 1)] {
            let y = top + offset;
            if y >= WORLD_HEIGHT {
                continue;
            }
            for dz in -radius..=radius {
                for dx in -radius..=radius {
                    // Clip the corners so the canopy reads round rather than cubic.
                    if dx.abs() == radius && dz.abs() == radius && radius > 1 {
                        continue;
                    }
                    let target = IVec3::new(local_x + dx, y, local_z + dz);
                    if self.get_opt(target) == Some(Block::Air) {
                        self.set_raw(target, Block::Leaves);
                    }
                }
            }
        }
    }

    fn refresh_max_height(&mut self) {
        self.max_height = 0;
        for y in (0..WORLD_HEIGHT).rev() {
            for z in 0..CHUNK_SIZE {
                for x in 0..CHUNK_SIZE {
                    if self.get(IVec3::new(x, y, z)).is_solid() {
                        self.max_height = y + 1;
                        return;
                    }
                }
            }
        }
    }

    /// Vertical-only skylight: full daylight down to the first opaque block, dark
    /// underneath. Column-independent, so it stays continuous across chunk
    /// borders for free. Horizontal softening happens per-vertex at mesh time.
    fn recompute_skylight(&mut self) {
        for z in 0..CHUNK_SIZE {
            for x in 0..CHUNK_SIZE {
                self.recompute_column_skylight(x, z);
            }
        }
    }

    fn recompute_column_skylight(&mut self, x: i32, z: i32) {
        let mut light = MAX_SKYLIGHT;
        for y in (0..WORLD_HEIGHT).rev() {
            let local = IVec3::new(x, y, z);
            if self.get(local).blocks_light() {
                light = 0;
            }
            if let Some(index) = self.index(local) {
                self.skylight[index] = light;
            }
        }
    }

    fn get(&self, local: IVec3) -> Block {
        self.get_opt(local).unwrap_or(Block::Air)
    }

    fn get_opt(&self, local: IVec3) -> Option<Block> {
        self.index(local).map(|index| self.blocks[index])
    }

    fn light(&self, local: IVec3) -> u8 {
        self.index(local).map(|index| self.skylight[index]).unwrap_or(MAX_SKYLIGHT)
    }

    fn set_raw(&mut self, local: IVec3, block: Block) -> bool {
        if let Some(index) = self.index(local) {
            self.blocks[index] = block;
            true
        } else {
            false
        }
    }

    fn set(&mut self, local: IVec3, block: Block) -> bool {
        if !self.set_raw(local, block) {
            return false;
        }
        self.recompute_column_skylight(local.x, local.z);
        if block.is_solid() {
            self.max_height = self.max_height.max(local.y + 1);
        } else if local.y + 1 >= self.max_height {
            self.refresh_max_height();
        }
        true
    }

    fn index(&self, local: IVec3) -> Option<usize> {
        if local.x < 0
            || local.x >= CHUNK_SIZE
            || local.y < 0
            || local.y >= WORLD_HEIGHT
            || local.z < 0
            || local.z >= CHUNK_SIZE
        {
            return None;
        }

        Some((local.x + local.z * CHUNK_SIZE + local.y * CHUNK_SIZE * CHUNK_SIZE) as usize)
    }
}

/// Blocks plus skylight for one chunk and a one-block skirt of its neighbours,
/// laid out as a flat array. Meshing reads this instead of the chunk HashMap,
/// which turns ~2.8M hashed lookups into 9 (one per source chunk).
struct Halo {
    blocks: Vec<Block>,
    skylight: Vec<u8>,
    height: i32,
}

impl Halo {
    fn index(x: i32, y: i32, z: i32) -> usize {
        ((x + PAD) + (z + PAD) * PADDED_SIZE + y * PADDED_SIZE * PADDED_SIZE) as usize
    }

    fn block(&self, x: i32, y: i32, z: i32) -> Block {
        if y < 0 || y >= WORLD_HEIGHT {
            return Block::Air;
        }
        self.blocks[Self::index(x, y, z)]
    }

    fn solid(&self, x: i32, y: i32, z: i32) -> bool {
        self.block(x, y, z).is_solid()
    }

    fn light(&self, x: i32, y: i32, z: i32) -> u8 {
        if y < 0 {
            return 0;
        }
        if y >= WORLD_HEIGHT {
            return MAX_SKYLIGHT;
        }
        self.skylight[Self::index(x, y, z)]
    }
}

/// Vertex data for one chunk, already packed for the GPU.
pub struct ChunkMesh {
    /// Two u32 per vertex, four vertices per quad.
    pub vertices: Vec<u32>,
    pub quad_count: usize,
}

pub struct World {
    chunks: HashMap<ChunkPosition, Chunk>,
    /// Player edits grouped by chunk so reloading a chunk does not scan every
    /// edit ever made.
    edits: HashMap<ChunkPosition, HashMap<IVec3, Block>>,
    noise: PerlinNoise,
    seed: u32,
    center: ChunkPosition,
    dirty: HashSet<ChunkPosition>,
}

impl World {
    pub fn generate(seed: u32) -> Self {
        let mut world = Self {
            chunks: HashMap::new(),
            edits: HashMap::new(),
            noise: PerlinNoise::new(seed),
            seed,
            center: ChunkPosition { x: 0, z: 0 },
            dirty: HashSet::new(),
        };
        world.load_chunks_around(Vec3::ZERO);
        world
    }

    pub fn load_chunks_around(&mut self, position: Vec3) -> bool {
        let next_center =
            ChunkPosition::from_world(position.x.floor() as i32, position.z.floor() as i32);
        let desired = chunk_positions_around(next_center);
        let changed = next_center != self.center
            || desired.len() != self.chunks.len()
            || desired
                .iter()
                .any(|position| !self.chunks.contains_key(position));

        if !changed {
            return false;
        }

        let removed: Vec<ChunkPosition> = self
            .chunks
            .keys()
            .filter(|position| !desired.contains(position))
            .copied()
            .collect();
        for position in &removed {
            self.chunks.remove(position);
            self.dirty.remove(position);
        }

        // Nearest first, so the ground under the player exists before anything
        // on the horizon does.
        let mut missing: Vec<ChunkPosition> = desired
            .iter()
            .filter(|position| !self.chunks.contains_key(position))
            .copied()
            .collect();
        missing.sort_by_key(|position| {
            let dx = position.x - next_center.x;
            let dz = position.z - next_center.z;
            dx * dx + dz * dz
        });

        // Capped per call: generating a whole window at once blocked the main
        // thread, both on the opening frame and every time the player crossed a
        // chunk border. The caller runs every frame, so the rest follows.
        for position in missing.into_iter().take(GENERATION_BUDGET) {
            let mut chunk = Chunk::generate(position, &self.noise, self.seed);
            if let Some(edits) = self.edits.get(&position) {
                for (&world_position, &block) in edits {
                    chunk.set_raw(local_position(world_position), block);
                }
                chunk.refresh_max_height();
                chunk.recompute_skylight();
            }
            self.chunks.insert(position, chunk);
            // The new chunk and its neighbours both gain or lose border faces.
            self.mark_dirty_with_neighbours(position);
        }
        self.center = next_center;
        true
    }

    pub fn loaded_chunk_count(&self) -> usize {
        self.chunks.len()
    }

    pub fn loaded_positions(&self) -> HashSet<ChunkPosition> {
        self.chunks.keys().copied().collect()
    }

    /// Chunks whose mesh is out of date. Draining leaves the world clean.
    pub fn take_dirty(&mut self) -> Vec<ChunkPosition> {
        self.dirty.drain().collect()
    }

    fn mark_dirty_with_neighbours(&mut self, position: ChunkPosition) {
        for dz in -1..=1 {
            for dx in -1..=1 {
                let neighbour = ChunkPosition {
                    x: position.x + dx,
                    z: position.z + dz,
                };
                if self.chunks.contains_key(&neighbour) {
                    self.dirty.insert(neighbour);
                }
            }
        }
    }

    pub fn get(&self, position: IVec3) -> Block {
        if position.y < 0 || position.y >= WORLD_HEIGHT {
            return Block::Air;
        }

        let chunk_position = ChunkPosition::from_world(position.x, position.z);
        self.chunks
            .get(&chunk_position)
            .map(|chunk| chunk.get(local_position(position)))
            .unwrap_or(Block::Air)
    }

    pub fn set(&mut self, position: IVec3, block: Block) -> bool {
        if position.y < 0 || position.y >= WORLD_HEIGHT {
            return false;
        }

        let chunk_position = ChunkPosition::from_world(position.x, position.z);
        let Some(chunk) = self.chunks.get_mut(&chunk_position) else {
            return false;
        };
        if !chunk.set(local_position(position), block) {
            return false;
        }

        self.edits
            .entry(chunk_position)
            .or_default()
            .insert(position, block);

        // A block on a chunk border also changes the neighbour's visible faces
        // and its ambient occlusion, so remesh those too.
        let local = local_position(position);
        self.dirty.insert(chunk_position);
        for dz in -1..=1 {
            for dx in -1..=1 {
                let touches_border = (dx < 0 && local.x == 0)
                    || (dx > 0 && local.x == CHUNK_SIZE - 1)
                    || (dz < 0 && local.z == 0)
                    || (dz > 0 && local.z == CHUNK_SIZE - 1);
                if (dx != 0 || dz != 0) && touches_border {
                    let neighbour = ChunkPosition {
                        x: chunk_position.x + dx,
                        z: chunk_position.z + dz,
                    };
                    if self.chunks.contains_key(&neighbour) {
                        self.dirty.insert(neighbour);
                    }
                }
            }
        }
        true
    }

    pub fn highest_block(&self, x: i32, z: i32) -> Option<i32> {
        (0..WORLD_HEIGHT)
            .rev()
            .find(|&y| self.get(IVec3::new(x, y, z)) != Block::Air)
    }

    pub fn is_solid(&self, position: IVec3) -> bool {
        self.get(position) != Block::Air
    }

    /// Amanatides & Woo voxel traversal: visits exactly the cells the ray
    /// crosses. Returns the hit cell and the normal of the face entered, which
    /// the fixed-step march could not provide (and which could miss corners).
    pub fn raycast(&self, origin: Vec3, direction: Vec3, reach: f32) -> Option<(IVec3, IVec3)> {
        let direction = direction.normalize_or_zero();
        if direction == Vec3::ZERO {
            return None;
        }

        let mut cell = origin.floor().as_ivec3();
        let step = IVec3::new(
            direction.x.signum() as i32,
            direction.y.signum() as i32,
            direction.z.signum() as i32,
        );

        // Distance along the ray between successive grid planes on each axis.
        let delta = Vec3::new(
            if direction.x == 0.0 { f32::INFINITY } else { (1.0 / direction.x).abs() },
            if direction.y == 0.0 { f32::INFINITY } else { (1.0 / direction.y).abs() },
            if direction.z == 0.0 { f32::INFINITY } else { (1.0 / direction.z).abs() },
        );

        let boundary = |start: f32, cell: i32, step: i32| -> f32 {
            if step > 0 {
                (cell + 1) as f32 - start
            } else {
                start - cell as f32
            }
        };
        let mut next = Vec3::new(
            if delta.x.is_finite() { boundary(origin.x, cell.x, step.x) * delta.x } else { f32::INFINITY },
            if delta.y.is_finite() { boundary(origin.y, cell.y, step.y) * delta.y } else { f32::INFINITY },
            if delta.z.is_finite() { boundary(origin.z, cell.z, step.z) * delta.z } else { f32::INFINITY },
        );

        if self.is_solid(cell) {
            return Some((cell, IVec3::ZERO));
        }

        let mut travelled = 0.0;
        while travelled <= reach {
            let normal;
            if next.x <= next.y && next.x <= next.z {
                travelled = next.x;
                cell.x += step.x;
                next.x += delta.x;
                normal = IVec3::new(-step.x, 0, 0);
            } else if next.y <= next.z {
                travelled = next.y;
                cell.y += step.y;
                next.y += delta.y;
                normal = IVec3::new(0, -step.y, 0);
            } else {
                travelled = next.z;
                cell.z += step.z;
                next.z += delta.z;
                normal = IVec3::new(0, 0, -step.z);
            }

            if travelled > reach {
                break;
            }
            if self.is_solid(cell) {
                return Some((cell, normal));
            }
        }

        None
    }

    /// Breaks the first block along the ray, returning it and the cell it was in
    /// so the caller can put it in hand.
    pub fn break_block(
        &mut self,
        origin: Vec3,
        direction: Vec3,
        reach: f32,
    ) -> Option<(IVec3, Block)> {
        let (cell, _) = self.raycast(origin, direction, reach)?;
        let block = self.get(cell);
        self.set(cell, Block::Air);
        Some((cell, block))
    }

    /// Places `block` against the face the ray enters.
    ///
    /// Refuses to fill a cell overlapping `player_min`/`player_max`, which would
    /// otherwise let the player seal themselves inside a block.
    pub fn place_block(
        &mut self,
        origin: Vec3,
        direction: Vec3,
        reach: f32,
        block: Block,
        player_min: Vec3,
        player_max: Vec3,
    ) -> Option<IVec3> {
        let (cell, normal) = self.raycast(origin, direction, reach)?;
        if normal == IVec3::ZERO {
            return None;
        }

        let target = cell + normal;
        if target.y < 0 || target.y >= WORLD_HEIGHT || self.get(target) != Block::Air {
            return None;
        }

        let overlaps = (target.x as f32) < player_max.x
            && (target.x + 1) as f32 > player_min.x
            && (target.y as f32) < player_max.y
            && (target.y + 1) as f32 > player_min.y
            && (target.z as f32) < player_max.z
            && (target.z + 1) as f32 > player_min.z;
        if overlaps {
            return None;
        }

        self.set(target, block).then_some(target)
    }

    fn build_halo(&self, position: ChunkPosition) -> Halo {
        let volume = (PADDED_SIZE * PADDED_SIZE * WORLD_HEIGHT) as usize;
        let mut halo = Halo {
            blocks: vec![Block::Air; volume],
            skylight: vec![MAX_SKYLIGHT; volume],
            height: 0,
        };

        // Nine source chunks, nine hashed lookups — everything after this is a
        // flat array read.
        for dz in -1..=1 {
            for dx in -1..=1 {
                let source = ChunkPosition {
                    x: position.x + dx,
                    z: position.z + dz,
                };
                let Some(chunk) = self.chunks.get(&source) else {
                    continue;
                };
                halo.height = halo.height.max(chunk.max_height);

                let (x_start, x_end) = span(dx);
                let (z_start, z_end) = span(dz);
                for y in 0..chunk.max_height {
                    for hz in z_start..=z_end {
                        for hx in x_start..=x_end {
                            let local = IVec3::new(
                                hx.rem_euclid(CHUNK_SIZE),
                                y,
                                hz.rem_euclid(CHUNK_SIZE),
                            );
                            let index = Halo::index(hx, y, hz);
                            halo.blocks[index] = chunk.get(local);
                            halo.skylight[index] = chunk.light(local);
                        }
                    }
                }
                // Above a chunk's max height everything is open sky.
                for y in chunk.max_height..WORLD_HEIGHT {
                    for hz in z_start..=z_end {
                        for hx in x_start..=x_end {
                            halo.skylight[Halo::index(hx, y, hz)] = MAX_SKYLIGHT;
                        }
                    }
                }
            }
        }

        halo
    }

    /// Builds the packed vertex buffer for one chunk, one quad per block face.
    ///
    /// Layout, two u32 per vertex:
    ///   word 0: x(8) | y(8) | z(8) | normal(3) | ao(2)
    ///   word 1: material(8) | skylight sum of the four touching blocks(6)
    ///
    /// The renderer uses the greedy variant below; this stays as the reference
    /// implementation the greedy one is checked against.
    #[cfg(test)]
    pub fn build_chunk_mesh(&self, position: ChunkPosition) -> ChunkMesh {
        let halo = self.build_halo(position);
        let mut vertices: Vec<u32> = Vec::new();
        let mut quad_count = 0;

        for y in 0..halo.height {
            for z in 0..CHUNK_SIZE {
                for x in 0..CHUNK_SIZE {
                    let block = halo.block(x, y, z);
                    if !block.is_solid() {
                        continue;
                    }

                    for (face_index, face) in FACES.iter().enumerate() {
                        let neighbour = IVec3::new(x, y, z) + face.neighbor;
                        if halo.solid(neighbour.x, neighbour.y, neighbour.z) {
                            continue;
                        }

                        let material = material_for(block, face_index);
                        let mut corners = [(0_u32, 0_u32); 4];
                        let mut ao = [0_u32; 4];

                        for (corner_index, corner) in face.corners.iter().enumerate() {
                            let (occlusion, light) =
                                corner_lighting(&halo, IVec3::new(x, y, z), face, *corner);
                            ao[corner_index] = occlusion;
                            corners[corner_index] = (
                                pack_position(
                                    x + corner[0],
                                    y + corner[1],
                                    z + corner[2],
                                    face_index as u32,
                                    occlusion,
                                ),
                                material | (light << 8),
                            );
                        }

                        // Rotating the quad flips its triangulation diagonal.
                        // Without this the well-known AO staircase artefact
                        // shows up on corners.
                        let rotate = ao[0] + ao[2] > ao[1] + ao[3];
                        for offset in 0..4 {
                            let index = if rotate { (offset + 1) % 4 } else { offset };
                            vertices.push(corners[index].0);
                            vertices.push(corners[index].1);
                        }
                        quad_count += 1;
                    }
                }
            }
        }

        ChunkMesh {
            vertices,
            quad_count,
        }
    }

    /// Same output as `build_chunk_mesh`, but coplanar faces that shade
    /// identically are merged into one larger quad.
    ///
    /// Merging is deliberately restricted: only faces whose ambient occlusion
    /// and skylight are uniform across all four corners are eligible. A face
    /// carrying a gradient is emitted alone, because stretching it over a
    /// merged rectangle would smear that gradient across the whole run.
    pub fn build_chunk_mesh_greedy(&self, position: ChunkPosition) -> ChunkMesh {
        let halo = self.build_halo(position);
        let mut vertices: Vec<u32> = Vec::new();
        let mut quad_count = 0;

        for (face_index, face) in FACES.iter().enumerate() {
            let axis = axis_of(face.neighbor);
            let u_axis = axis_of(face.tangent_u);
            let v_axis = axis_of(face.tangent_v);
            let axis_len = axis_extent(axis, halo.height);
            let u_len = axis_extent(u_axis, halo.height);
            let v_len = axis_extent(v_axis, halo.height);

            let mut mask = vec![MaskCell::Empty; (u_len * v_len) as usize];
            // Reused across slices: reallocating these per slice cost more than
            // the merging saved.
            let mut gradients: Vec<(u32, [(u32, u32); 4])> = Vec::new();

            for slice in 0..axis_len {
                gradients.clear();
                mask.fill(MaskCell::Empty);

                for v in 0..v_len {
                    for u in 0..u_len {
                        let block_position = compose(axis, slice, u_axis, u, v_axis, v);
                        let block = halo.block(block_position.x, block_position.y, block_position.z);
                        if !block.is_solid() {
                            continue;
                        }
                        let neighbour = block_position + face.neighbor;
                        if halo.solid(neighbour.x, neighbour.y, neighbour.z) {
                            continue;
                        }

                        let material = material_for(block, face_index);
                        let mut corners = [(0_u32, 0_u32); 4];
                        for (index, corner) in face.corners.iter().enumerate() {
                            corners[index] = corner_lighting(&halo, block_position, face, *corner);
                        }

                        let uniform = corners.iter().all(|corner| *corner == corners[0]);
                        mask[(u + v * u_len) as usize] = if uniform {
                            MaskCell::Mergeable {
                                material,
                                ao: corners[0].0,
                                light: corners[0].1,
                            }
                        } else {
                            gradients.push((material, corners));
                            MaskCell::Unique(gradients.len() - 1)
                        };
                    }
                }

                // Sweep the mask, growing each rectangle as far right then as
                // far down as identical cells allow.
                let mut v = 0;
                while v < v_len {
                    let mut u = 0;
                    while u < u_len {
                        let cell = mask[(u + v * u_len) as usize];
                        if cell == MaskCell::Empty {
                            u += 1;
                            continue;
                        }

                        let mut width = 1;
                        if matches!(cell, MaskCell::Mergeable { .. }) {
                            while u + width < u_len
                                && mask[(u + width + v * u_len) as usize] == cell
                            {
                                width += 1;
                            }
                        }

                        let mut height = 1;
                        if matches!(cell, MaskCell::Mergeable { .. }) {
                            'grow: while v + height < v_len {
                                for offset in 0..width {
                                    if mask[(u + offset + (v + height) * u_len) as usize] != cell {
                                        break 'grow;
                                    }
                                }
                                height += 1;
                            }
                        }

                        for row in 0..height {
                            for column in 0..width {
                                mask[(u + column + (v + row) * u_len) as usize] = MaskCell::Empty;
                            }
                        }

                        let origin = compose(axis, slice, u_axis, u, v_axis, v);
                        let (material, corner_data) = match cell {
                            MaskCell::Mergeable {
                                material,
                                ao,
                                light,
                            } => (material, [(ao, light); 4]),
                            MaskCell::Unique(index) => gradients[index],
                            MaskCell::Empty => unreachable!(),
                        };

                        emit_quad(
                            &mut vertices,
                            face,
                            face_index,
                            origin,
                            u_axis,
                            v_axis,
                            width,
                            height,
                            material,
                            corner_data,
                        );
                        quad_count += 1;
                        u += width;
                    }
                    v += 1;
                }
            }
        }

        ChunkMesh {
            vertices,
            quad_count,
        }
    }
}

/// Writes one quad, stretched by `width`/`height` along the face tangents.
/// Stretching preserves the corner order from `FACES`, so the winding stays
/// correct for back-face culling.
#[allow(clippy::too_many_arguments)]
fn emit_quad(
    vertices: &mut Vec<u32>,
    face: &Face,
    face_index: usize,
    origin: IVec3,
    u_axis: usize,
    v_axis: usize,
    width: i32,
    height: i32,
    material: u32,
    corners: [(u32, u32); 4],
) {
    let mut packed = [(0_u32, 0_u32); 4];
    for (index, corner) in face.corners.iter().enumerate() {
        let mut position = origin;
        position[u_axis] += corner[u_axis] * width;
        position[v_axis] += corner[v_axis] * height;
        // The axis normal to the face is never stretched.
        let axis = axis_of(face.neighbor);
        position[axis] = origin[axis] + corner[axis];

        let (ao, light) = corners[index];
        packed[index] = (
            pack_position(position.x, position.y, position.z, face_index as u32, ao),
            material | (light << 8),
        );
    }

    let rotate = corners[0].0 + corners[2].0 > corners[1].0 + corners[3].0;
    for offset in 0..4 {
        let index = if rotate { (offset + 1) % 4 } else { offset };
        vertices.push(packed[index].0);
        vertices.push(packed[index].1);
    }
}

/// One cell of the greedy sweep mask.
#[derive(Clone, Copy, PartialEq, Eq)]
enum MaskCell {
    Empty,
    /// Ambient occlusion and skylight are uniform across the face, so merging
    /// it with an identical neighbour cannot change how it shades.
    Mergeable {
        material: u32,
        ao: u32,
        light: u32,
    },
    /// The face has a gradient across its corners; stretching it would smear
    /// that gradient, so it is emitted on its own.
    Unique(usize),
}

fn axis_of(vector: IVec3) -> usize {
    if vector.x != 0 {
        0
    } else if vector.y != 0 {
        1
    } else {
        2
    }
}

fn axis_extent(axis: usize, height: i32) -> i32 {
    if axis == 1 {
        height
    } else {
        CHUNK_SIZE
    }
}

fn compose(axis: usize, a: i32, u_axis: usize, u: i32, v_axis: usize, v: i32) -> IVec3 {
    let mut position = IVec3::ZERO;
    position[axis] = a;
    position[u_axis] = u;
    position[v_axis] = v;
    position
}

fn span(delta: i32) -> (i32, i32) {
    match delta {
        -1 => (-PAD, -1),
        0 => (0, CHUNK_SIZE - 1),
        _ => (CHUNK_SIZE, CHUNK_SIZE + PAD - 1),
    }
}

fn pack_position(x: i32, y: i32, z: i32, normal: u32, ao: u32) -> u32 {
    (x as u32 & 0xFF)
        | ((y as u32 & 0xFF) << 8)
        | ((z as u32 & 0xFF) << 16)
        | ((normal & 0x7) << 24)
        | ((ao & 0x3) << 27)
}

/// Standard voxel ambient occlusion: a corner darkens with the number of solid
/// blocks touching it. Skylight is averaged over the same four blocks, which
/// smooths light horizontally without needing a flood fill.
fn corner_lighting(halo: &Halo, block: IVec3, face: &Face, corner: [i32; 3]) -> (u32, u32) {
    let front = block + face.neighbor;
    let u = face.tangent_u * (corner_sign(corner, face.tangent_u));
    let v = face.tangent_v * (corner_sign(corner, face.tangent_v));

    let side_u = front + u;
    let side_v = front + v;
    let diagonal = front + u + v;

    let solid_u = halo.solid(side_u.x, side_u.y, side_u.z);
    let solid_v = halo.solid(side_v.x, side_v.y, side_v.z);
    let solid_diagonal = halo.solid(diagonal.x, diagonal.y, diagonal.z);

    let occlusion = if solid_u && solid_v {
        0
    } else {
        3 - (solid_u as u32 + solid_v as u32 + solid_diagonal as u32)
    };

    // Only open blocks contribute light; a solid neighbour would drag the
    // average to zero and darken every edge.
    let mut total = 0_u32;
    let mut samples = 0_u32;
    for position in [front, side_u, side_v, diagonal] {
        if !halo.solid(position.x, position.y, position.z) {
            total += halo.light(position.x, position.y, position.z) as u32;
            samples += 1;
        }
    }
    let light = if samples == 0 {
        0
    } else {
        (total * 4) / samples / 4
    };

    (occlusion, light.min(MAX_SKYLIGHT as u32))
}

/// +1 when the corner sits on the positive side of the given tangent axis.
fn corner_sign(corner: [i32; 3], tangent: IVec3) -> i32 {
    let component = if tangent.x != 0 {
        corner[0]
    } else if tangent.y != 0 {
        corner[1]
    } else {
        corner[2]
    };
    component * 2 - 1
}

fn chunk_positions_around(center: ChunkPosition) -> HashSet<ChunkPosition> {
    let mut positions = HashSet::with_capacity(((RENDER_DISTANCE * 2 + 1).pow(2)) as usize);
    for z in -RENDER_DISTANCE..=RENDER_DISTANCE {
        for x in -RENDER_DISTANCE..=RENDER_DISTANCE {
            positions.insert(ChunkPosition {
                x: center.x + x,
                z: center.z + z,
            });
        }
    }
    positions
}

fn local_position(world: IVec3) -> IVec3 {
    IVec3::new(
        world.x.rem_euclid(CHUNK_SIZE),
        world.y,
        world.z.rem_euclid(CHUNK_SIZE),
    )
}

fn terrain_height(noise: &PerlinNoise, world_x: i32, world_z: i32) -> i32 {
    let x = world_x as f32;
    let z = world_z as f32;

    let broad = noise.octave_sample(x * 0.018, z * 0.018, 4);
    // Keep the detail octave low and smooth: higher frequencies scatter
    // one-block steps across the surface and the terrain stops reading as ground.
    let detail = noise.octave_sample(x * 0.045 + 20.0, z * 0.045, 2);

    // A low-frequency mask keeps mountains local: most of the map stays rolling
    // hills, and only a few regions lift into ridges.
    let mask = noise.sample(x * 0.0055 + 100.0, z * 0.0055 - 40.0);
    let mountain = ((mask - 0.08).max(0.0) / 0.92).powf(1.4);
    let ridge = noise.ridge_sample(x * 0.021 - 60.0, z * 0.021 + 15.0, 3);

    let height = 14.0 + broad * 6.0 + detail * 1.8 + mountain * ridge * 34.0;
    height.round().clamp(3.0, (WORLD_HEIGHT - 7) as f32) as i32
}

fn surface_block(height: i32, y: i32) -> Block {
    let depth = height - y;
    let coastal = height <= SEA_LEVEL + 1;

    if depth == 0 {
        if coastal {
            Block::Sand
        } else if height >= 33 {
            Block::Snow
        } else if height >= 27 {
            Block::Stone
        } else {
            Block::Grass
        }
    } else if depth <= 3 {
        if coastal {
            Block::Sand
        } else if height >= 27 {
            Block::Stone
        } else {
            Block::Dirt
        }
    } else {
        Block::Stone
    }
}

/// Deterministic tree placement. Returns the trunk height when a tree grows at
/// this world column.
fn tree_at(seed: u32, noise: &PerlinNoise, world_x: i32, world_z: i32) -> Option<i32> {
    let ground = terrain_height(noise, world_x, world_z);
    if surface_block(ground, ground) != Block::Grass {
        return None;
    }
    if ground + 8 >= WORLD_HEIGHT {
        return None;
    }

    let hash = hash_2d(seed, world_x, world_z);
    // Roughly one column in a hundred, which reads as a sparse forest.
    if hash % 100 >= 3 {
        return None;
    }

    Some(4 + ((hash >> 8) % 3) as i32)
}

fn hash_2d(seed: u32, x: i32, z: i32) -> u32 {
    let mut hash = seed ^ 0x9E37_79B9;
    hash = hash.wrapping_add((x as u32).wrapping_mul(0x85EB_CA6B));
    hash ^= hash >> 13;
    hash = hash.wrapping_add((z as u32).wrapping_mul(0xC2B2_AE35));
    hash ^= hash >> 16;
    hash = hash.wrapping_mul(0x27D4_EB2F);
    hash ^ (hash >> 15)
}

const FACE_POSITIVE_Y: usize = 2;

struct Face {
    neighbor: IVec3,
    corners: [[i32; 3]; 4],
    tangent_u: IVec3,
    tangent_v: IVec3,
}

const FACES: [Face; 6] = [
    // +X
    Face {
        neighbor: IVec3::new(1, 0, 0),
        corners: [[1, 0, 0], [1, 1, 0], [1, 1, 1], [1, 0, 1]],
        tangent_u: IVec3::new(0, 1, 0),
        tangent_v: IVec3::new(0, 0, 1),
    },
    // -X
    Face {
        neighbor: IVec3::new(-1, 0, 0),
        corners: [[0, 0, 1], [0, 1, 1], [0, 1, 0], [0, 0, 0]],
        tangent_u: IVec3::new(0, 1, 0),
        tangent_v: IVec3::new(0, 0, 1),
    },
    // +Y
    Face {
        neighbor: IVec3::new(0, 1, 0),
        corners: [[0, 1, 0], [0, 1, 1], [1, 1, 1], [1, 1, 0]],
        tangent_u: IVec3::new(1, 0, 0),
        tangent_v: IVec3::new(0, 0, 1),
    },
    // -Y
    Face {
        neighbor: IVec3::new(0, -1, 0),
        corners: [[0, 0, 1], [0, 0, 0], [1, 0, 0], [1, 0, 1]],
        tangent_u: IVec3::new(1, 0, 0),
        tangent_v: IVec3::new(0, 0, 1),
    },
    // +Z
    Face {
        neighbor: IVec3::new(0, 0, 1),
        corners: [[1, 0, 1], [1, 1, 1], [0, 1, 1], [0, 0, 1]],
        tangent_u: IVec3::new(1, 0, 0),
        tangent_v: IVec3::new(0, 1, 0),
    },
    // -Z
    Face {
        neighbor: IVec3::new(0, 0, -1),
        corners: [[0, 0, 0], [0, 1, 0], [1, 1, 0], [1, 0, 0]],
        tangent_u: IVec3::new(1, 0, 0),
        tangent_v: IVec3::new(0, 1, 0),
    },
];

#[cfg(test)]
mod tests {
    use super::{
        Block, ChunkPosition, World, CHUNK_SIZE, RENDER_DISTANCE, WORLD_HEIGHT,
    };
    use glam::{IVec3, Vec3};

    #[test]
    fn generated_world_contains_all_terrain_materials() {
        let world = World::generate(1_337);
        let mut seen = [false; 8];
        for y in 0..WORLD_HEIGHT {
            for z in -CHUNK_SIZE..CHUNK_SIZE {
                for x in -CHUNK_SIZE..CHUNK_SIZE {
                    seen[world.get(IVec3::new(x, y, z)) as usize] = true;
                }
            }
        }
        assert!(seen[Block::Grass as usize]);
        assert!(seen[Block::Dirt as usize]);
        assert!(seen[Block::Stone as usize]);
        assert!(seen[Block::Sand as usize]);
    }

    /// Chunks stream in over several calls, so tests that need a full window
    /// have to pump until it settles.
    fn settle(world: &mut World, position: Vec3) {
        for _ in 0..64 {
            if !world.load_chunks_around(position) {
                return;
            }
        }
        panic!("chunk loading never settled");
    }

    #[test]
    fn chunk_window_follows_the_player() {
        let mut world = World::generate(21);
        let expected = ((RENDER_DISTANCE * 2 + 1).pow(2)) as usize;

        settle(&mut world, Vec3::ZERO);
        assert_eq!(world.loaded_chunk_count(), expected);

        let far = Vec3::new(CHUNK_SIZE as f32 * 5.0, 0.0, 0.0);
        assert!(world.load_chunks_around(far), "moving must load new chunks");
        settle(&mut world, far);
        assert_eq!(world.loaded_chunk_count(), expected);
        assert!(world.highest_block(CHUNK_SIZE * 5, 0).is_some());
    }

    /// Generating a whole window in one call used to block the frame; the work
    /// is now spread out, nearest chunks first.
    #[test]
    fn chunk_generation_is_spread_across_calls() {
        let world = World::generate(23);
        assert!(
            world.loaded_chunk_count() <= super::GENERATION_BUDGET,
            "one call generated {} chunks",
            world.loaded_chunk_count()
        );
        // The player spawns at the origin, so that chunk must come first.
        assert!(
            world.highest_block(0, 0).is_some(),
            "the spawn chunk must be generated before distant ones"
        );
    }

    #[test]
    fn edits_survive_chunk_reloading() {
        let mut world = World::generate(8);
        let position = IVec3::new(0, 20, 0);
        world.set(position, Block::Stone);
        world.load_chunks_around(Vec3::new(CHUNK_SIZE as f32 * 8.0, 0.0, 0.0));
        world.load_chunks_around(Vec3::ZERO);
        assert_eq!(world.get(position), Block::Stone);
    }

    #[test]
    fn raycast_removes_the_first_block() {
        let mut world = World::generate(9);
        let position = IVec3::new(4, 20, 4);
        world.set(position, Block::Stone);
        let broken = world.break_block(Vec3::new(4.5, 20.5, 0.5), Vec3::Z, 5.0);
        assert_eq!(broken, Some((position, Block::Stone)));
        assert_eq!(world.get(position), Block::Air);
    }

    /// A player box far away from the test area, so placement is not refused
    /// for the wrong reason.
    fn distant_player() -> (Vec3, Vec3) {
        (
            Vec3::new(100.0, 100.0, 100.0),
            Vec3::new(100.6, 101.8, 100.6),
        )
    }

    #[test]
    fn place_block_lands_on_the_face_the_ray_enters() {
        let mut world = World::generate(31);
        let anchor = IVec3::new(4, 25, 6);
        world.set(anchor, Block::Stone);

        let (min, max) = distant_player();
        let placed = world.place_block(
            Vec3::new(4.5, 25.5, 0.5),
            Vec3::Z,
            8.0,
            Block::Sand,
            min,
            max,
        );

        // The ray travels along +Z, so it enters the -Z face and the new block
        // goes one step back towards the camera.
        assert_eq!(placed, Some(IVec3::new(4, 25, 5)));
        assert_eq!(world.get(IVec3::new(4, 25, 5)), Block::Sand);
    }

    /// Firing from inside a block yields no entered face, so there is no
    /// sensible cell to place against.
    #[test]
    fn place_block_refuses_when_the_ray_starts_inside_a_block() {
        let mut world = World::generate(32);
        let inside = IVec3::new(4, 25, 5);
        world.set(inside, Block::Stone);

        let (min, max) = distant_player();
        let placed = world.place_block(
            Vec3::new(4.5, 25.5, 5.5),
            Vec3::Z,
            8.0,
            Block::Sand,
            min,
            max,
        );
        assert_eq!(placed, None);
    }

    #[test]
    fn place_block_refuses_above_the_world_ceiling() {
        let mut world = World::generate(35);
        let top = IVec3::new(4, WORLD_HEIGHT - 1, 4);
        world.set(top, Block::Stone);

        let (min, max) = distant_player();
        // Look down onto the topmost block from above: its upper face points at
        // a cell outside the world.
        let placed = world.place_block(
            Vec3::new(4.5, WORLD_HEIGHT as f32 + 2.0, 4.5),
            -Vec3::Y,
            8.0,
            Block::Sand,
            min,
            max,
        );
        assert_eq!(placed, None);
    }

    #[test]
    fn place_block_refuses_to_seal_the_player_in() {
        let mut world = World::generate(33);
        let anchor = IVec3::new(4, 25, 6);
        world.set(anchor, Block::Stone);

        // Player standing exactly where the block would go.
        let min = Vec3::new(3.7, 25.0, 4.7);
        let max = Vec3::new(4.3, 26.8, 5.3);
        let placed = world.place_block(
            Vec3::new(4.5, 25.5, 0.5),
            Vec3::Z,
            8.0,
            Block::Sand,
            min,
            max,
        );

        assert_eq!(placed, None, "placing inside the player must be refused");
        assert_eq!(world.get(IVec3::new(4, 25, 5)), Block::Air);
    }

    #[test]
    fn break_block_reports_what_was_broken() {
        let mut world = World::generate(34);
        let position = IVec3::new(4, 25, 4);
        world.set(position, Block::Sand);

        let broken = world.break_block(Vec3::new(4.5, 25.5, 0.5), Vec3::Z, 8.0);
        assert_eq!(broken, Some((position, Block::Sand)));
    }

    #[test]
    fn raycast_reports_the_entered_face() {
        let mut world = World::generate(11);
        let position = IVec3::new(4, 25, 4);
        world.set(position, Block::Stone);

        let (cell, normal) = world
            .raycast(Vec3::new(4.5, 25.5, 0.5), Vec3::Z, 8.0)
            .expect("ray should hit the block");
        assert_eq!(cell, position);
        assert_eq!(normal, IVec3::new(0, 0, -1));
    }

    #[test]
    fn raycast_does_not_tunnel_through_diagonal_corners() {
        let mut world = World::generate(12);
        // A solid wall the ray must not slip past between two sample points.
        for y in 24..27 {
            for x in 2..7 {
                world.set(IVec3::new(x, y, 5), Block::Stone);
            }
        }
        let hit = world.raycast(
            Vec3::new(2.1, 24.1, 2.0),
            Vec3::new(0.35, 0.12, 1.0),
            12.0,
        );
        assert!(hit.is_some(), "DDA must not step over the wall");
        assert_eq!(hit.unwrap().0.z, 5);
    }

    #[test]
    fn breaking_a_border_block_marks_the_neighbour_dirty() {
        let mut world = World::generate(5);
        world.take_dirty();

        let border = IVec3::new(0, 20, 0);
        world.set(border, Block::Stone);
        let dirty = world.take_dirty();

        assert!(dirty.contains(&ChunkPosition { x: 0, z: 0 }));
        assert!(
            dirty.contains(&ChunkPosition { x: -1, z: 0 }),
            "the chunk sharing this border must be remeshed too"
        );
    }

    #[test]
    fn chunk_mesh_emits_four_vertices_per_quad() {
        let world = World::generate(3);
        let mesh = world.build_chunk_mesh(ChunkPosition { x: 0, z: 0 });
        assert!(mesh.quad_count > 0);
        // Two u32 per vertex, four vertices per quad.
        assert_eq!(mesh.vertices.len(), mesh.quad_count * 4 * 2);
    }

    #[test]
    fn trees_are_not_cut_at_chunk_borders() {
        let world = World::generate(1_337);
        let mut leaves = 0;
        let mut wood = 0;
        for y in 0..WORLD_HEIGHT {
            for z in -CHUNK_SIZE..CHUNK_SIZE * 2 {
                for x in -CHUNK_SIZE..CHUNK_SIZE * 2 {
                    match world.get(IVec3::new(x, y, z)) {
                        Block::Leaves => leaves += 1,
                        Block::Wood => wood += 1,
                        _ => {}
                    }
                }
            }
        }
        assert!(wood > 0, "the seed should grow at least one tree");
        // A complete canopy carries far more leaves than trunk blocks; a canopy
        // clipped at a border would collapse this ratio.
        assert!(
            leaves > wood * 4,
            "canopies look truncated: {leaves} leaves for {wood} wood"
        );
    }

    #[test]
    fn skylight_is_dark_under_solid_ground() {
        let world = World::generate(17);
        let surface = world.highest_block(3, 3).expect("column should have ground");
        let chunk = world
            .chunks
            .get(&ChunkPosition { x: 0, z: 0 })
            .expect("origin chunk is loaded");
        assert_eq!(chunk.light(IVec3::new(3, surface, 3)), 0);
        assert_eq!(chunk.light(IVec3::new(3, surface + 1, 3)), super::MAX_SKYLIGHT);
    }

    #[test]
    fn max_height_tracks_the_tallest_block() {
        let mut world = World::generate(4);
        let position = IVec3::new(2, 40, 2);
        world.set(position, Block::Stone);
        {
            let chunk = world.chunks.get(&ChunkPosition { x: 0, z: 0 }).unwrap();
            assert_eq!(chunk.max_height, 41);
        }

        world.set(position, Block::Air);
        let chunk = world.chunks.get(&ChunkPosition { x: 0, z: 0 }).unwrap();
        assert!(chunk.max_height < 41, "removing the top block must lower max_height");
    }

    #[test]
    fn terrain_produces_both_lowlands_and_mountains() {
        let noise = super::PerlinNoise::new(1_337);
        let mut lowest = i32::MAX;
        let mut highest = i32::MIN;
        // Sample wide enough to cross at least one mountain mask region.
        for z in (-600..600).step_by(7) {
            for x in (-600..600).step_by(7) {
                let height = super::terrain_height(&noise, x, z);
                lowest = lowest.min(height);
                highest = highest.max(height);
            }
        }
        assert!(
            lowest <= super::SEA_LEVEL,
            "expected coastline, lowest was {lowest}"
        );
        assert!(
            highest >= 30,
            "expected mountains somewhere, highest was {highest}"
        );
        assert!(highest < WORLD_HEIGHT, "terrain must stay under the ceiling");
    }

    /// Total surface a mesh covers, in unit block faces. Greedy merging must
    /// not change it: fewer quads, same geometry.
    fn covered_area(mesh: &super::ChunkMesh) -> i64 {
        let mut area = 0;
        for quad in mesh.vertices.chunks(8) {
            let mut min = [i32::MAX; 3];
            let mut max = [i32::MIN; 3];
            for vertex in quad.chunks(2) {
                let word = vertex[0];
                let position = [
                    (word & 0xFF) as i32,
                    ((word >> 8) & 0xFF) as i32,
                    ((word >> 16) & 0xFF) as i32,
                ];
                for axis in 0..3 {
                    min[axis] = min[axis].min(position[axis]);
                    max[axis] = max[axis].max(position[axis]);
                }
            }
            let extents: Vec<i64> = (0..3)
                .map(|axis| (max[axis] - min[axis]) as i64)
                .filter(|extent| *extent > 0)
                .collect();
            area += extents.iter().product::<i64>().max(1);
        }
        area
    }

    #[test]
    fn greedy_meshing_covers_exactly_the_same_surface() {
        let world = World::generate(1_337);
        for position in world.loaded_positions() {
            let plain = world.build_chunk_mesh(position);
            let greedy = world.build_chunk_mesh_greedy(position);

            assert_eq!(
                covered_area(&plain),
                covered_area(&greedy),
                "greedy meshing changed the covered surface at {position:?}"
            );
            assert!(
                greedy.quad_count <= plain.quad_count,
                "greedy meshing must never emit more quads"
            );
            assert_eq!(greedy.vertices.len(), greedy.quad_count * 4 * 2);
        }
    }

    /// Not part of the normal suite. Run with:
    ///   cargo test --release -- --ignored --nocapture
    #[test]
    #[ignore]
    fn mesh_benchmark() {
        let world = World::generate(1_337);
        let positions: Vec<_> = world.loaded_positions().into_iter().collect();

        let start = std::time::Instant::now();
        let mut quads = 0;
        let mut words = 0;
        for &position in &positions {
            let mesh = world.build_chunk_mesh(position);
            quads += mesh.quad_count;
            words += mesh.vertices.len();
        }
        let full = start.elapsed();

        let single = std::time::Instant::now();
        let _ = world.build_chunk_mesh(positions[0]);
        let single = single.elapsed();

        println!(
            "BENCH plain   chunks={} quads={} vertex_bytes={:.2}MB all={:?} one={:?}",
            positions.len(),
            quads,
            (words * 4) as f64 / 1_048_576.0,
            full,
            single
        );

        let start = std::time::Instant::now();
        let mut greedy_quads = 0;
        let mut greedy_words = 0;
        for &position in &positions {
            let mesh = world.build_chunk_mesh_greedy(position);
            greedy_quads += mesh.quad_count;
            greedy_words += mesh.vertices.len();
        }
        let greedy_full = start.elapsed();

        let single = std::time::Instant::now();
        let _ = world.build_chunk_mesh_greedy(positions[0]);
        let greedy_single = single.elapsed();

        println!(
            "BENCH greedy  chunks={} quads={} vertex_bytes={:.2}MB all={:?} one={:?}",
            positions.len(),
            greedy_quads,
            (greedy_words * 4) as f64 / 1_048_576.0,
            greedy_full,
            greedy_single
        );
        println!(
            "BENCH verdict quads {:.1}% of plain, meshing {:.1}% of plain",
            greedy_quads as f64 / quads as f64 * 100.0,
            greedy_full.as_secs_f64() / full.as_secs_f64() * 100.0
        );
    }
}


#[cfg(test)]
mod generation_cost {
    use super::*;
    #[test]
    #[ignore]
    fn measure() {
        let start = std::time::Instant::now();
        let world = World::generate(1_337);
        let full = start.elapsed();
        let n = world.loaded_chunk_count();

        let mut w2 = World::generate(1_337);
        let start = std::time::Instant::now();
        w2.load_chunks_around(Vec3::new(CHUNK_SIZE as f32, 0.0, 0.0));
        let step = start.elapsed();
        println!("GEN {n} chunks in {full:?} ({:?}/chunk), crossing a border: {step:?}",
            full / n as u32);
    }
}
