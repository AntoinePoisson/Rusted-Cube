# Rusted Cube

[![Lighthouse performance 100](https://img.shields.io/badge/Lighthouse_Performance-100-brightgreen)](#performance)
[![Lighthouse accessibility 100](https://img.shields.io/badge/Accessibility-100-brightgreen)](#performance)
[![Lighthouse best practices 100](https://img.shields.io/badge/Best_Practices-100-brightgreen)](#performance)
[![Lighthouse SEO 100](https://img.shields.io/badge/SEO-100-brightgreen)](#performance)

Rusted Cube is a small voxel sandbox that runs in the browser. The terrain, game loop, player physics, collision detection, block raycasting and mesh generation are written in Rust and compiled to WebAssembly. Rendering uses the browser's WebGL2 API directly, without Three.js or another 3D engine.

![The Rusted Cube title screen over a generated forest](docs/screenshot-title.png)

## Features

- Endless chunked terrain using a custom multi-octave Perlin noise implementation
- Ridged mountains gated by a low-frequency mask, so peaks stay local instead of lifting the whole map
- Deterministic trees that spill across chunk borders without being clipped
- Grass, dirt, sand, stone, snow, wood and leaf blocks
- Per-vertex ambient occlusion and vertical skylight, both baked at mesh time
- A day/night cycle driving sun direction, sun colour, sky colour and fog
- First-person movement, jumping, sprinting and solid-block collisions
- Block breaking and placing with a DDA voxel raycast, on a shared cooldown
- The targeted block is outlined, so aiming is not guesswork
- Optional LAN multiplayer: a bundled server shares the world with everyone on
  the network, and the game still runs single-player without it
- Persistent block edits when a chunk is unloaded and revisited
- Greedy voxel meshing with per-chunk GPU buffers and frustum culling
- Live performance readout: frame rate, meshing cost, visible chunks, triangle count

![In-game view showing the crosshair and first-person hand](docs/screenshot-game.png)

## Run locally

You need Rust, the `wasm32-unknown-unknown` target, `wasm-pack` and a small static file server.

```sh
rustup target add wasm32-unknown-unknown
wasm-pack build --target web --release
python3 -m http.server 8080
```

Open [http://localhost:8080](http://localhost:8080) and click the game to capture the cursor.

## Deploying

Pushing to `main` or `prod` builds the WebAssembly bundle and publishes it to GitHub Pages
(`.github/workflows/deploy-pages.yml`). Tests must pass first — a red build is never
deployed. The workflow can also be triggered by hand from the Actions tab.

Enable it once under **Settings → Pages → Source → GitHub Actions**.

The published site is single-player. GitHub Pages serves static files only, so there is no
WebSocket endpoint, and the page knows it: no connection is attempted. LAN play needs the
bundled server below.

The workflow also inlines `styles.css` into the page, which is why the deployed site scores
higher than a local run.

The workflow pins Rust to 1.76 rather than tracking `stable`, because Rust 1.82 changed the
WebAssembly ABI in a way that breaks the wasm-bindgen release this project uses.

## Play together on a local network

Instead of the static file server, run the one in `server/`. It serves the page and the
WebSocket from a single process:

```sh
wasm-pack build --target web --release
cd server && cargo run --release        # optional: pass a seed, e.g. `-- 42`
```

It prints the address to share. The server has **no dependencies at all**: the HTTP
responses, the WebSocket handshake and its frame codec are written on `std`, one thread
per connection. For a few players on a local network that is simpler than an async
runtime, and it builds on the same compiler as the game.

Everyone on the same network then opens `http://<your-lan-ip>:8118` — the server prints
that address on startup. Port 8118 rather than 8080, which is already taken far too often.

The server owns the world seed and the list of block edits; clients generate the terrain
locally from that seed, so only player poses and edits travel over the wire.

The connection is optional, and the page is told which case it is in rather than finding
out the hard way: the server rewrites `data-multiplayer="0"` to `"1"` in the HTML it
serves. Behind any other static host the attribute stays `0`, no socket is opened, and the
game runs single-player. `R` is disabled while connected, since the world is shared.

## Controls

| Action | Input |
| --- | --- |
| Move | `W`, `A`, `S`, `D` or arrow keys |
| Look | Mouse |
| Jump | `Space` |
| Sprint | Left `Shift` |
| Break a block | Left click |
| Place the block you hold | Right click |
| Generate a new world | `R` (single-player only) |
| Release the cursor | `Escape` |

## Project layout

```text
src/
├── game.rs       Browser events, game loop and frame budgeting
├── input.rs      Input state
├── net.rs        WebSocket client, optional
├── perlin.rs     Seeded 2D Perlin and ridged noise
├── player.rs     Camera, movement and collisions
├── protocol.rs   Wire format, encoded and parsed by hand
├── renderer.rs   WebGL2 renderer, frustum culling and shaders
└── world.rs      Chunks, terrain, lighting, raycasting and meshing

server/
├── src/main.rs      HTTP, connection threads, world state
├── src/websocket.rs Handshake and frame codec
└── src/sha1.rs      Only used to answer the handshake
```

## Technical notes

The world keeps a 7×7 window of chunks around the player. Chunks outside that window are
unloaded, while player edits are stored per chunk and reapplied when a chunk returns.

**Meshing.** Each chunk owns its own vertex buffer and is remeshed only when marked dirty,
so breaking a block rebuilds one chunk rather than the whole window. Editing a block on a
chunk border also marks the neighbour, since its visible faces and ambient occlusion change
too. Meshing reads from a flat padded copy of the chunk and its eight neighbours, which
costs nine hashed lookups per chunk and turns everything after that into array reads.
Coplanar faces that shade identically are merged into larger quads; faces carrying an
ambient-occlusion gradient are left alone, because stretching them would smear that
gradient across the run.

**Lighting.** Ambient occlusion is computed per vertex from the three blocks touching each
corner, and the quad's triangulation diagonal is flipped when the occlusion is anisotropic
to avoid the usual staircase artefact. Skylight is vertical per column, which keeps it
continuous across chunk borders for free, and is averaged over the four blocks touching a
vertex to soften it horizontally. Both are packed into the vertex, so lighting costs the
GPU nothing at draw time. There is no shadow mapping: the sun moves, but cast shadows do
not follow it.

**Vertex format.** Two `u32` per vertex — position, normal index and ambient occlusion in
the first word, material and skylight in the second — unpacked in the vertex shader. Chunk
positions are local, with the chunk origin supplied as a uniform. A single immutable index
buffer is shared by every chunk.

Measured on the default seed, release build: 29 197 quads across 49 chunks, 0.89 MB of
vertex data, and roughly 0.4 ms to mesh one chunk.

Run the meshing benchmark with:

```sh
cargo test --release -- --ignored --nocapture
```

**Multiplayer.** The server is authoritative over the seed and the edit list, and relays
player poses; terrain is never transmitted. Poses are sent 20 times a second and
interpolated on the receiving side. The protocol is JSON encoded and parsed by hand
(`src/protocol.rs`), shared with the server by including the same source file, so neither
side needs a serialisation crate. Messages arriving off the network are rejected rather
than trusted when malformed.

## Performance

Lighthouse against a production build — the deployed layout, served compressed, as GitHub
Pages does:

| Category | Score |
| --- | --- |
| Performance | 100 |
| Accessibility | 100 |
| Best Practices | 100 |
| SEO | 100 |

Reproduce against the bundled server with:

```sh
npx lighthouse@11 http://localhost:8118/index.html --view
```

Note that this measures the development layout: the stylesheet is a separate request and
nothing is compressed, so it scores a point or two lower than the deployed site.

What the score cost, each measured rather than assumed:

- **Meshing spread across frames.** Building all 49 chunks up front blocked the main
  thread for **2,870 ms**; over frames it is **~70 ms**. Score 68 → 98.
- **Chunk generation spread too**, nearest first. This also removes the stall that used to
  happen whenever the player crossed a chunk border.
- **The sky is painted before the world is generated.** The canvas is the largest element
  on the page, so leaving it blank delayed the largest contentful paint by about a second.
- **The loader is a badge, not a curtain.** Covering the page hid the content it was
  waiting for.
- **The stylesheet is inlined at deploy time**, removing a render-blocking round trip.
  `styles.css` remains the single source of truth; the workflow inlines it.
- **Font sizes on narrow screens** raised above 12px, which is what SEO was docking.
- **No socket is opened when the host cannot serve one** — see below. A failed WebSocket
  handshake is logged as a console error, and that alone cost Best Practices 4 points.

None of this changed the in-game frame rate, which is the figure that actually matters
while playing.

PWA scores 38 and is not pursued: this is a local sandbox, not an installable app.

## Mobile

The page **renders** on a phone — the world draws, the layout holds, and the HUD drops its
profiling figures on narrow screens — but the game is **not playable there**. Looking
around needs pointer lock, moving needs `W`/`A`/`S`/`D`, and placing needs a right click.
None of that exists on a touch screen. Playable mobile support would mean on-screen sticks
and tap-to-break, which is not implemented.

## Not implemented

- Touch controls, so phones can render the world but not play it
- Cast shadows from the moving sun
- Transparent blocks, which would need depth sorting

## License

This project is available under the MIT License.
