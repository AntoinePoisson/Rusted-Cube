use std::collections::HashMap;

use glam::{IVec3, Mat4, Vec3, Vec4};
use wasm_bindgen::{JsCast, JsValue};
use web_sys::{
    HtmlCanvasElement, WebGl2RenderingContext as Gl, WebGlBuffer, WebGlProgram, WebGlShader,
    WebGlUniformLocation, WebGlVertexArrayObject,
};

use crate::world::{ChunkPosition, CHUNK_SIZE, MAX_QUADS_PER_CHUNK, RENDER_DISTANCE, WORLD_HEIGHT};

const VERTEX_SIZE: i32 = 8;

struct ChunkMesh {
    vertex_array: WebGlVertexArrayObject,
    vertex_buffer: WebGlBuffer,
    index_count: i32,
    origin: Vec3,
}

pub struct Renderer {
    gl: Gl,
    canvas: HtmlCanvasElement,
    program: WebGlProgram,
    index_buffer: WebGlBuffer,
    chunks: HashMap<ChunkPosition, ChunkMesh>,
    view_projection_uniform: WebGlUniformLocation,
    camera_uniform: WebGlUniformLocation,
    chunk_origin_uniform: WebGlUniformLocation,
    sun_direction_uniform: WebGlUniformLocation,
    sun_color_uniform: WebGlUniformLocation,
    sky_color_uniform: WebGlUniformLocation,
    fog_range_uniform: WebGlUniformLocation,
    // Chunk positions are packed into local bytes, so entities need their own pipeline.
    entity_program: WebGlProgram,
    entity_vertex_array: WebGlVertexArrayObject,
    outline_vertex_array: WebGlVertexArrayObject,
    entity_view_projection_uniform: WebGlUniformLocation,
    entity_model_uniform: WebGlUniformLocation,
    entity_color_uniform: WebGlUniformLocation,
    entity_sun_direction_uniform: WebGlUniformLocation,
    pixel_ratio_cap: f64,
    view_projection: Mat4,
    drawn_triangles: i32,
    visible_chunks: usize,
}

impl Renderer {
    pub fn new(canvas: HtmlCanvasElement) -> Result<Self, JsValue> {
        let gl = canvas
            .get_context("webgl2")?
            .ok_or_else(|| JsValue::from_str("WebGL2 is not available in this browser"))?
            .dyn_into::<Gl>()?;

        let vertex_shader = compile_shader(&gl, Gl::VERTEX_SHADER, VERTEX_SHADER)?;
        let fragment_shader = compile_shader(&gl, Gl::FRAGMENT_SHADER, FRAGMENT_SHADER)?;
        let program = link_program(&gl, &vertex_shader, &fragment_shader)?;

        let index_buffer = build_shared_index_buffer(&gl)?;

        let uniform = |name: &str| -> Result<WebGlUniformLocation, JsValue> {
            gl.get_uniform_location(&program, name)
                .ok_or_else(|| JsValue::from_str(&format!("Missing uniform {name}")))
        };
        let view_projection_uniform = uniform("u_view_projection")?;
        let camera_uniform = uniform("u_camera")?;
        let chunk_origin_uniform = uniform("u_chunk_origin")?;
        let sun_direction_uniform = uniform("u_sun_direction")?;
        let sun_color_uniform = uniform("u_sun_color")?;
        let sky_color_uniform = uniform("u_sky_color")?;
        let fog_range_uniform = uniform("u_fog_range")?;

        let entity_vertex = compile_shader(&gl, Gl::VERTEX_SHADER, ENTITY_VERTEX_SHADER)?;
        let entity_fragment = compile_shader(&gl, Gl::FRAGMENT_SHADER, ENTITY_FRAGMENT_SHADER)?;
        let entity_program = link_program(&gl, &entity_vertex, &entity_fragment)?;
        let entity_vertex_array = build_unit_cube(&gl, &entity_program)?;
        let outline_vertex_array = build_cube_outline(&gl, &entity_program)?;

        let entity_uniform = |name: &str| -> Result<WebGlUniformLocation, JsValue> {
            gl.get_uniform_location(&entity_program, name)
                .ok_or_else(|| JsValue::from_str(&format!("Missing entity uniform {name}")))
        };
        let entity_view_projection_uniform = entity_uniform("u_view_projection")?;
        let entity_model_uniform = entity_uniform("u_model")?;
        let entity_color_uniform = entity_uniform("u_color")?;
        let entity_sun_direction_uniform = entity_uniform("u_sun_direction")?;

        gl.enable(Gl::DEPTH_TEST);
        gl.enable(Gl::CULL_FACE);
        gl.cull_face(Gl::BACK);

        let pixel_ratio_cap = web_sys::window()
            .and_then(|window| window.document())
            .and_then(|document| document.document_element())
            .and_then(|root| root.get_attribute("data-input"))
            .map_or(2.0, |input| if input == "touch" { 1.5 } else { 2.0 });

        Ok(Self {
            gl,
            canvas,
            program,
            index_buffer,
            chunks: HashMap::new(),
            view_projection_uniform,
            camera_uniform,
            chunk_origin_uniform,
            sun_direction_uniform,
            sun_color_uniform,
            sky_color_uniform,
            fog_range_uniform,
            entity_program,
            entity_vertex_array,
            outline_vertex_array,
            entity_view_projection_uniform,
            entity_model_uniform,
            entity_color_uniform,
            entity_sun_direction_uniform,
            pixel_ratio_cap,
            view_projection: Mat4::IDENTITY,
            drawn_triangles: 0,
            visible_chunks: 0,
        })
    }

    pub fn upload_chunk(&mut self, position: ChunkPosition, vertices: &[u32], quad_count: usize) {
        if quad_count == 0 {
            self.drop_chunk(position);
            return;
        }

        let mesh = match self.chunks.get(&position) {
            Some(existing) => existing,
            None => {
                let Some(mesh) = self.create_chunk_mesh(position) else {
                    return;
                };
                self.chunks.insert(position, mesh);
                &self.chunks[&position]
            }
        };

        self.gl.bind_vertex_array(Some(&mesh.vertex_array));
        self.gl
            .bind_buffer(Gl::ARRAY_BUFFER, Some(&mesh.vertex_buffer));
        unsafe {
            let array = js_sys::Uint32Array::view(vertices);
            self.gl
                .buffer_data_with_array_buffer_view(Gl::ARRAY_BUFFER, &array, Gl::STATIC_DRAW);
        }
        self.gl.bind_vertex_array(None);

        if let Some(mesh) = self.chunks.get_mut(&position) {
            mesh.index_count = (quad_count.min(MAX_QUADS_PER_CHUNK) * 6) as i32;
        }
    }

    fn create_chunk_mesh(&self, position: ChunkPosition) -> Option<ChunkMesh> {
        let vertex_array = self.gl.create_vertex_array()?;
        let vertex_buffer = self.gl.create_buffer()?;

        self.gl.bind_vertex_array(Some(&vertex_array));
        self.gl.bind_buffer(Gl::ARRAY_BUFFER, Some(&vertex_buffer));
        self.gl.enable_vertex_attrib_array(0);
        self.gl
            .vertex_attrib_i_pointer_with_i32(0, 1, Gl::UNSIGNED_INT, VERTEX_SIZE, 0);
        self.gl.enable_vertex_attrib_array(1);
        self.gl
            .vertex_attrib_i_pointer_with_i32(1, 1, Gl::UNSIGNED_INT, VERTEX_SIZE, 4);
        self.gl
            .bind_buffer(Gl::ELEMENT_ARRAY_BUFFER, Some(&self.index_buffer));
        self.gl.bind_vertex_array(None);

        Some(ChunkMesh {
            vertex_array,
            vertex_buffer,
            index_count: 0,
            origin: Vec3::new(
                (position.x * CHUNK_SIZE) as f32,
                0.0,
                (position.z * CHUNK_SIZE) as f32,
            ),
        })
    }

    pub fn drop_chunk(&mut self, position: ChunkPosition) {
        if let Some(mesh) = self.chunks.remove(&position) {
            self.gl.delete_buffer(Some(&mesh.vertex_buffer));
            self.gl.delete_vertex_array(Some(&mesh.vertex_array));
        }
    }

    pub fn retain_chunks(&mut self, keep: impl Fn(ChunkPosition) -> bool) {
        let stale: Vec<ChunkPosition> = self
            .chunks
            .keys()
            .filter(|position| !keep(**position))
            .copied()
            .collect();
        for position in stale {
            self.drop_chunk(position);
        }
    }

    pub fn clear_chunks(&mut self) {
        self.retain_chunks(|_| false);
    }

    pub fn drawn_triangles(&self) -> i32 {
        self.drawn_triangles
    }

    pub fn visible_chunks(&self) -> usize {
        self.visible_chunks
    }

    pub fn present_sky(&mut self, sky: &SkyState) {
        self.resize_to_display();
        self.gl
            .clear_color(sky.sky_color.x, sky.sky_color.y, sky.sky_color.z, 1.0);
        self.gl.clear(Gl::COLOR_BUFFER_BIT | Gl::DEPTH_BUFFER_BIT);
    }

    /// Matches the drawing buffer to the display, capped at 2x pixel density.
    fn resize_to_display(&mut self) -> (u32, u32) {
        let width = self.canvas.client_width().max(1) as u32;
        let height = self.canvas.client_height().max(1) as u32;
        let pixel_ratio = web_sys::window()
            .map(|window| window.device_pixel_ratio())
            .unwrap_or(1.0)
            .min(self.pixel_ratio_cap);
        let buffer_width = (width as f64 * pixel_ratio) as u32;
        let buffer_height = (height as f64 * pixel_ratio) as u32;

        if self.canvas.width() != buffer_width || self.canvas.height() != buffer_height {
            self.canvas.set_width(buffer_width);
            self.canvas.set_height(buffer_height);
        }
        self.gl
            .viewport(0, 0, buffer_width as i32, buffer_height as i32);
        (buffer_width, buffer_height)
    }

    pub fn render(&mut self, eye: Vec3, direction: Vec3, sky: &SkyState) {
        let (buffer_width, buffer_height) = self.resize_to_display();
        self.gl
            .clear_color(sky.sky_color.x, sky.sky_color.y, sky.sky_color.z, 1.0);
        self.gl.clear(Gl::COLOR_BUFFER_BIT | Gl::DEPTH_BUFFER_BIT);
        self.gl.use_program(Some(&self.program));

        let far = far_plane();
        let projection = Mat4::perspective_rh_gl(
            72_f32.to_radians(),
            buffer_width as f32 / buffer_height as f32,
            0.05,
            far,
        );
        let view = Mat4::look_at_rh(eye, eye + direction, Vec3::Y);
        let view_projection = projection * view;
        let frustum = Frustum::from_matrix(view_projection);

        self.gl.uniform_matrix4fv_with_f32_array(
            Some(&self.view_projection_uniform),
            false,
            &view_projection.to_cols_array(),
        );
        self.gl
            .uniform3f(Some(&self.camera_uniform), eye.x, eye.y, eye.z);
        self.gl.uniform3f(
            Some(&self.sun_direction_uniform),
            sky.sun_direction.x,
            sky.sun_direction.y,
            sky.sun_direction.z,
        );
        self.gl.uniform3f(
            Some(&self.sun_color_uniform),
            sky.sun_color.x,
            sky.sun_color.y,
            sky.sun_color.z,
        );
        self.gl.uniform3f(
            Some(&self.sky_color_uniform),
            sky.sky_color.x,
            sky.sky_color.y,
            sky.sky_color.z,
        );
        let fog_start = far * 0.45;
        self.gl
            .uniform2f(Some(&self.fog_range_uniform), fog_start, far * 0.95);

        self.view_projection = view_projection;
        self.drawn_triangles = 0;
        self.visible_chunks = 0;

        for mesh in self.chunks.values() {
            if mesh.index_count == 0 {
                continue;
            }
            let min = mesh.origin;
            let max = min + Vec3::new(CHUNK_SIZE as f32, WORLD_HEIGHT as f32, CHUNK_SIZE as f32);
            if !frustum.intersects_aabb(min, max) {
                continue;
            }

            self.gl.uniform3f(
                Some(&self.chunk_origin_uniform),
                mesh.origin.x,
                mesh.origin.y,
                mesh.origin.z,
            );
            self.gl.bind_vertex_array(Some(&mesh.vertex_array));
            self.gl
                .draw_elements_with_i32(Gl::TRIANGLES, mesh.index_count, Gl::UNSIGNED_INT, 0);

            self.drawn_triangles += mesh.index_count / 3;
            self.visible_chunks += 1;
        }
        self.gl.bind_vertex_array(None);
    }

    fn begin_entity_pass(&self, sky: &SkyState) {
        self.gl.use_program(Some(&self.entity_program));
        self.gl.uniform_matrix4fv_with_f32_array(
            Some(&self.entity_view_projection_uniform),
            false,
            &self.view_projection.to_cols_array(),
        );
        self.gl.uniform3f(
            Some(&self.entity_sun_direction_uniform),
            sky.sun_direction.x,
            sky.sun_direction.y,
            sky.sun_direction.z,
        );
    }

    pub fn render_block_highlight(&self, cell: IVec3, sky: &SkyState) {
        self.begin_entity_pass(sky);
        self.gl.bind_vertex_array(Some(&self.outline_vertex_array));

        // A small scale offset keeps the lines in front of the block faces.
        let center = Vec3::new(cell.x as f32, cell.y as f32, cell.z as f32) + Vec3::splat(0.5);
        let model = Mat4::from_translation(center) * Mat4::from_scale(Vec3::splat(1.006));
        self.gl.uniform_matrix4fv_with_f32_array(
            Some(&self.entity_model_uniform),
            false,
            &model.to_cols_array(),
        );
        self.gl
            .uniform3f(Some(&self.entity_color_uniform), 0.02, 0.02, 0.03);
        self.gl.draw_arrays(Gl::LINES, 0, 24);
        self.gl.bind_vertex_array(None);
    }

    pub fn render_avatars(&self, avatars: &[Avatar], sky: &SkyState) {
        if avatars.is_empty() {
            return;
        }

        self.begin_entity_pass(sky);
        self.gl.bind_vertex_array(Some(&self.entity_vertex_array));

        for avatar in avatars {
            let rotation = Mat4::from_rotation_y(std::f32::consts::FRAC_PI_2 - avatar.yaw);
            let translation = Mat4::from_translation(avatar.position);

            for part in &BODY {
                let model = translation
                    * rotation
                    * Mat4::from_translation(part.center)
                    * Mat4::from_scale(part.half * 2.0);
                self.gl.uniform_matrix4fv_with_f32_array(
                    Some(&self.entity_model_uniform),
                    false,
                    &model.to_cols_array(),
                );
                let color = avatar.color * part.shade;
                self.gl
                    .uniform3f(Some(&self.entity_color_uniform), color.x, color.y, color.z);
                self.gl.draw_arrays(Gl::TRIANGLES, 0, 36);
            }
        }
        self.gl.bind_vertex_array(None);
    }
}

struct BodyPart {
    center: Vec3,
    half: Vec3,
    shade: f32,
}

const BODY: [BodyPart; 6] = [
    BodyPart {
        center: Vec3::new(0.0, 1.55, 0.0),
        half: Vec3::new(0.25, 0.25, 0.25),
        shade: 1.0,
    },
    BodyPart {
        center: Vec3::new(0.0, 1.05, 0.0),
        half: Vec3::new(0.25, 0.25, 0.125),
        shade: 0.9,
    },
    BodyPart {
        center: Vec3::new(-0.375, 1.05, 0.0),
        half: Vec3::new(0.125, 0.25, 0.125),
        shade: 0.82,
    },
    BodyPart {
        center: Vec3::new(0.375, 1.05, 0.0),
        half: Vec3::new(0.125, 0.25, 0.125),
        shade: 0.82,
    },
    BodyPart {
        center: Vec3::new(-0.125, 0.4, 0.0),
        half: Vec3::new(0.125, 0.4, 0.125),
        shade: 0.7,
    },
    BodyPart {
        center: Vec3::new(0.125, 0.4, 0.0),
        half: Vec3::new(0.125, 0.4, 0.125),
        shade: 0.7,
    },
];

pub struct Avatar {
    pub position: Vec3,
    pub yaw: f32,
    pub color: Vec3,
}

fn far_plane() -> f32 {
    let horizontal = ((RENDER_DISTANCE + 1) * CHUNK_SIZE) as f32 * std::f32::consts::SQRT_2;
    (horizontal + WORLD_HEIGHT as f32).ceil()
}

pub struct SkyState {
    pub sun_direction: Vec3,
    pub sun_color: Vec3,
    pub sky_color: Vec3,
}

impl SkyState {
    /// `time_of_day` runs 0..1 over a cycle, 0.25 is noon.
    pub fn at(time_of_day: f32) -> Self {
        let angle = time_of_day * std::f32::consts::TAU;
        let sun_direction =
            Vec3::new(angle.cos() * 0.6, angle.sin(), angle.cos() * 0.35).normalize_or_zero();

        let elevation = sun_direction.y.max(0.0);
        let dawn = (1.0 - elevation).powf(2.0);

        let day_sky = Vec3::new(0.48, 0.72, 0.90);
        let dusk_sky = Vec3::new(0.86, 0.52, 0.34);
        let night_sky = Vec3::new(0.05, 0.07, 0.13);

        let sky_color = if sun_direction.y > 0.0 {
            day_sky.lerp(dusk_sky, dawn) * (0.35 + 0.65 * elevation.powf(0.5))
                + night_sky * (1.0 - elevation).powf(3.0)
        } else {
            let dusk = (1.0 + sun_direction.y * 6.0).clamp(0.0, 1.0);
            night_sky.lerp(dusk_sky * 0.45, dusk)
        };

        let sun_color =
            Vec3::new(1.0, 0.96, 0.88).lerp(Vec3::new(1.0, 0.72, 0.45), dawn) * elevation.powf(0.4);

        Self {
            sun_direction,
            sun_color,
            sky_color,
        }
    }
}

/// Gribb & Hartmann plane extraction. Each plane is (a, b, c, d) with the normal
/// pointing inwards.
struct Frustum {
    planes: [Vec4; 6],
}

impl Frustum {
    fn from_matrix(matrix: Mat4) -> Self {
        let m = matrix.to_cols_array_2d();
        let row = |index: usize| Vec4::new(m[0][index], m[1][index], m[2][index], m[3][index]);
        let (x, y, z, w) = (row(0), row(1), row(2), row(3));

        let mut planes = [w + x, w - x, w + y, w - y, w + z, w - z];
        for plane in &mut planes {
            let length = Vec3::new(plane.x, plane.y, plane.z).length();
            if length > 0.0 {
                *plane /= length;
            }
        }

        Self { planes }
    }

    fn intersects_aabb(&self, min: Vec3, max: Vec3) -> bool {
        for plane in &self.planes {
            let positive = Vec3::new(
                if plane.x >= 0.0 { max.x } else { min.x },
                if plane.y >= 0.0 { max.y } else { min.y },
                if plane.z >= 0.0 { max.z } else { min.z },
            );
            if plane.x * positive.x + plane.y * positive.y + plane.z * positive.z + plane.w < 0.0 {
                return false;
            }
        }
        true
    }
}

fn build_shared_index_buffer(gl: &Gl) -> Result<WebGlBuffer, JsValue> {
    let buffer = gl
        .create_buffer()
        .ok_or_else(|| JsValue::from_str("Could not create the index buffer"))?;

    let mut indices: Vec<u32> = Vec::with_capacity(MAX_QUADS_PER_CHUNK * 6);
    for quad in 0..MAX_QUADS_PER_CHUNK as u32 {
        let base = quad * 4;
        indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }

    gl.bind_buffer(Gl::ELEMENT_ARRAY_BUFFER, Some(&buffer));
    unsafe {
        let array = js_sys::Uint32Array::view(&indices);
        gl.buffer_data_with_array_buffer_view(Gl::ELEMENT_ARRAY_BUFFER, &array, Gl::STATIC_DRAW);
    }
    gl.bind_buffer(Gl::ELEMENT_ARRAY_BUFFER, None);

    Ok(buffer)
}

fn build_cube_outline(gl: &Gl, program: &WebGlProgram) -> Result<WebGlVertexArrayObject, JsValue> {
    const CORNERS: [[f32; 3]; 8] = [
        [-0.5, -0.5, -0.5],
        [0.5, -0.5, -0.5],
        [0.5, -0.5, 0.5],
        [-0.5, -0.5, 0.5],
        [-0.5, 0.5, -0.5],
        [0.5, 0.5, -0.5],
        [0.5, 0.5, 0.5],
        [-0.5, 0.5, 0.5],
    ];
    const EDGES: [(usize, usize); 12] = [
        (0, 1),
        (1, 2),
        (2, 3),
        (3, 0),
        (4, 5),
        (5, 6),
        (6, 7),
        (7, 4),
        (0, 4),
        (1, 5),
        (2, 6),
        (3, 7),
    ];

    let mut data: Vec<f32> = Vec::with_capacity(24 * 6);
    for (from, to) in EDGES {
        for corner in [CORNERS[from], CORNERS[to]] {
            data.extend_from_slice(&corner);
            data.extend_from_slice(&[0.0, 1.0, 0.0]);
        }
    }

    upload_entity_geometry(gl, program, &data)
}

fn build_unit_cube(gl: &Gl, program: &WebGlProgram) -> Result<WebGlVertexArrayObject, JsValue> {
    const FACES: [([f32; 3], [f32; 3], [f32; 3]); 6] = [
        ([1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]),
        ([-1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, -1.0]),
        ([0.0, 1.0, 0.0], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]),
        ([0.0, -1.0, 0.0], [0.0, 0.0, -1.0], [1.0, 0.0, 0.0]),
        ([0.0, 0.0, 1.0], [0.0, 1.0, 0.0], [-1.0, 0.0, 0.0]),
        ([0.0, 0.0, -1.0], [0.0, 1.0, 0.0], [1.0, 0.0, 0.0]),
    ];

    let mut data: Vec<f32> = Vec::with_capacity(36 * 6);
    for (normal, up, right) in FACES {
        let corner = |u: f32, v: f32| {
            [
                (normal[0] + up[0] * u + right[0] * v) * 0.5,
                (normal[1] + up[1] * u + right[1] * v) * 0.5,
                (normal[2] + up[2] * u + right[2] * v) * 0.5,
            ]
        };
        for (u, v) in [
            (-1.0, -1.0),
            (-1.0, 1.0),
            (1.0, 1.0),
            (-1.0, -1.0),
            (1.0, 1.0),
            (1.0, -1.0),
        ] {
            data.extend_from_slice(&corner(u, v));
            data.extend_from_slice(&normal);
        }
    }

    upload_entity_geometry(gl, program, &data)
}

fn upload_entity_geometry(
    gl: &Gl,
    program: &WebGlProgram,
    data: &[f32],
) -> Result<WebGlVertexArrayObject, JsValue> {
    let vertex_array = gl
        .create_vertex_array()
        .ok_or_else(|| JsValue::from_str("Could not create the entity vertex array"))?;
    let buffer = gl
        .create_buffer()
        .ok_or_else(|| JsValue::from_str("Could not create the entity buffer"))?;

    gl.bind_vertex_array(Some(&vertex_array));
    gl.bind_buffer(Gl::ARRAY_BUFFER, Some(&buffer));
    unsafe {
        let array = js_sys::Float32Array::view(data);
        gl.buffer_data_with_array_buffer_view(Gl::ARRAY_BUFFER, &array, Gl::STATIC_DRAW);
    }

    let stride = 6 * std::mem::size_of::<f32>() as i32;
    let position = gl.get_attrib_location(program, "a_position") as u32;
    let normal = gl.get_attrib_location(program, "a_normal") as u32;
    gl.enable_vertex_attrib_array(position);
    gl.vertex_attrib_pointer_with_i32(position, 3, Gl::FLOAT, false, stride, 0);
    gl.enable_vertex_attrib_array(normal);
    gl.vertex_attrib_pointer_with_i32(normal, 3, Gl::FLOAT, false, stride, 12);
    gl.bind_vertex_array(None);

    Ok(vertex_array)
}

fn compile_shader(gl: &Gl, shader_type: u32, source: &str) -> Result<WebGlShader, JsValue> {
    let shader = gl
        .create_shader(shader_type)
        .ok_or_else(|| JsValue::from_str("Could not create a WebGL shader"))?;
    gl.shader_source(&shader, source);
    gl.compile_shader(&shader);

    if gl
        .get_shader_parameter(&shader, Gl::COMPILE_STATUS)
        .as_bool()
        .unwrap_or(false)
    {
        Ok(shader)
    } else {
        Err(JsValue::from_str(
            &gl.get_shader_info_log(&shader)
                .unwrap_or_else(|| "Unknown shader error".to_owned()),
        ))
    }
}

fn link_program(
    gl: &Gl,
    vertex_shader: &WebGlShader,
    fragment_shader: &WebGlShader,
) -> Result<WebGlProgram, JsValue> {
    let program = gl
        .create_program()
        .ok_or_else(|| JsValue::from_str("Could not create the WebGL program"))?;
    gl.attach_shader(&program, vertex_shader);
    gl.attach_shader(&program, fragment_shader);
    gl.link_program(&program);

    if gl
        .get_program_parameter(&program, Gl::LINK_STATUS)
        .as_bool()
        .unwrap_or(false)
    {
        Ok(program)
    } else {
        Err(JsValue::from_str(
            &gl.get_program_info_log(&program)
                .unwrap_or_else(|| "Unknown shader link error".to_owned()),
        ))
    }
}

const VERTEX_SHADER: &str = r#"#version 300 es
precision highp float;

// word0: x(8) | y(8) | z(8) | normal(3) | ao(2)
// word1: material(8) | skylight(6)
layout(location = 0) in uint a_word0;
layout(location = 1) in uint a_word1;

uniform mat4 u_view_projection;
uniform vec3 u_chunk_origin;

out vec3 v_world_position;
out vec3 v_normal;
out vec3 v_color;
out float v_ao;
out float v_skylight;

const vec3 NORMALS[6] = vec3[6](
    vec3( 1.0,  0.0,  0.0),
    vec3(-1.0,  0.0,  0.0),
    vec3( 0.0,  1.0,  0.0),
    vec3( 0.0, -1.0,  0.0),
    vec3( 0.0,  0.0,  1.0),
    vec3( 0.0,  0.0, -1.0)
);

// GrassTop, GrassSide, Dirt, Stone, Sand, Wood, Leaves, Snow
const vec3 PALETTE[8] = vec3[8](
    vec3(0.34, 0.62, 0.22),
    vec3(0.42, 0.34, 0.18),
    vec3(0.45, 0.32, 0.18),
    vec3(0.42, 0.44, 0.45),
    vec3(0.76, 0.69, 0.45),
    vec3(0.35, 0.25, 0.15),
    vec3(0.22, 0.47, 0.19),
    vec3(0.92, 0.94, 0.97)
);

void main() {
    vec3 local = vec3(
        float(a_word0 & 0xFFu),
        float((a_word0 >> 8) & 0xFFu),
        float((a_word0 >> 16) & 0xFFu)
    );
    uint normal_index = (a_word0 >> 24) & 0x7u;
    uint ao = (a_word0 >> 27) & 0x3u;
    uint material = a_word1 & 0xFFu;
    uint skylight = (a_word1 >> 8) & 0x3Fu;

    vec3 world_position = u_chunk_origin + local;
    v_world_position = world_position;
    v_normal = NORMALS[normal_index];
    v_color = PALETTE[material];
    v_ao = float(ao) / 3.0;
    v_skylight = float(skylight) / 15.0;

    gl_Position = u_view_projection * vec4(world_position, 1.0);
}
"#;

const FRAGMENT_SHADER: &str = r#"#version 300 es
precision highp float;

in vec3 v_world_position;
in vec3 v_normal;
in vec3 v_color;
in float v_ao;
in float v_skylight;

uniform vec3 u_camera;
uniform vec3 u_sun_direction;
uniform vec3 u_sun_color;
uniform vec3 u_sky_color;
uniform vec2 u_fog_range;

out vec4 out_color;

void main() {
    // per-face tint, keeps cube edges readable in flat light
    float face = 0.78;
    if (v_normal.y > 0.5) {
        face = 1.0;
    } else if (v_normal.y < -0.5) {
        face = 0.62;
    } else if (abs(v_normal.x) > 0.5) {
        face = 0.86;
    }

    // half lambert, a hard max(dot, 0) drops faces turned away from the sun to
    // pure black and they read as holes
    float lambert = dot(v_normal, u_sun_direction) * 0.5 + 0.5;
    float diffuse = lambert * lambert;
    float exposure = mix(0.25, 1.0, v_skylight);
    // never full black or corners stop being legible
    float occlusion = mix(0.6, 1.0, v_ao);

    // pulled back towards white, taking the sky color neat washes every
    // material towards blue-green
    vec3 ambient = mix(vec3(1.0), u_sky_color, 0.6) * 0.55 * exposure;
    vec3 direct = u_sun_color * diffuse * 0.65 * exposure;
    vec3 shaded = v_color * face * occlusion * (ambient + direct + 0.05);

    float distance_to_camera = distance(v_world_position, u_camera);
    float fog = smoothstep(u_fog_range.x, u_fog_range.y, distance_to_camera);
    out_color = vec4(mix(shaded, u_sky_color, fog), 1.0);
}
"#;

const ENTITY_VERTEX_SHADER: &str = r#"#version 300 es
precision highp float;

in vec3 a_position;
in vec3 a_normal;

uniform mat4 u_view_projection;
uniform mat4 u_model;

out vec3 v_normal;

void main() {
    // parts are only translated, rotated about Y and scaled, so the upper block
    // of the model matrix is fine to use on the normal directly
    v_normal = normalize(mat3(u_model) * a_normal);
    gl_Position = u_view_projection * u_model * vec4(a_position, 1.0);
}
"#;

const ENTITY_FRAGMENT_SHADER: &str = r#"#version 300 es
precision highp float;

in vec3 v_normal;

uniform vec3 u_color;
uniform vec3 u_sun_direction;

out vec4 out_color;

void main() {
    float lambert = dot(normalize(v_normal), u_sun_direction) * 0.5 + 0.5;
    out_color = vec4(u_color * (0.55 + 0.45 * lambert * lambert), 1.0);
}
"#;
