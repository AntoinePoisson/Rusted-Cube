mod blockee;

use three_d::*;

#[cfg(not(target_arch = "wasm32"))]
#[tokio::main]
#[allow(dead_code)]
async fn main() {
    run().await;
}

#[allow(dead_code)]
pub async fn run() {
    let window = Window::new(WindowSettings {
        title: "Instanced Shapes!".to_string(),
        max_size: Some((1280, 720)),
        ..Default::default()
    })
    .unwrap();
    let context = window.gl();

    let mut camera = Camera::new_perspective(
        window.viewport(),
        vec3(60.00, 50.0, 60.0), // camera position
        vec3(0.0, 0.0, 0.0),     // camera target
        vec3(0.0, 1.0, 0.0),     // camera up
        degrees(45.0),
        0.1,
        1000.0,
    );

    let mut control = FirstPersonControl::new(1.0);
    // let mut control = OrbitControl::new(vec3(0.0, 0.0, 0.0), 1.0, 1000.0);

    let light0 = DirectionalLight::new(&context, 1.0, Color::WHITE, &vec3(0.0, -0.5, -0.5));
    let light1 = DirectionalLight::new(&context, 1.0, Color::WHITE, &vec3(0.0, 0.5, 0.5));

    let mut loaded = three_d_asset::io::load_async(&["src/resources/block.png"])
        .await
        .unwrap();

    // Container for non instanced meshes.
    let mut non_instanced_meshes = Vec::new();

    // Instanced mesh object, initialise with empty instances.
    let mut instanced_mesh = Gm::new(
        InstancedMesh::new(&context, &Instances::default(), &CpuMesh::cube()),
        ColorMaterial {
            texture: Some(
                std::sync::Arc::new(Texture2D::new(
                    &context,
                    &loaded.deserialize("block").unwrap(),
                ))
                .into(),
            ),
            ..Default::default()
        },
        // // PhysicalMaterial::new(
        // //     &context,
        // //     &CpuMaterial {
        // //         albedo: Color {
        // //             r: 128,
        // //             g: 128,
        // //             b: 128,
        // //             a: 255,
        // //         },
        // //         ..Default::default()
        // //     },
        // // ),
    );
    instanced_mesh.set_animation(|time| Mat4::from_angle_x(Rad(time)));

    // Create a CPU-side mesh consisting of a single colored triangle
    // let positions = vec![
    //     Vec3::new(1.0, -1.0, -1.0),
    //     Vec3::new(1.0, 1.0, -1.0),
    //     Vec3::new(1.0, 1.0, 1.0),
    //     Vec3::new(1.0, 1.0, 1.0),
    //     Vec3::new(1.0, -1.0, 1.0),
    //     Vec3::new(1.0, -1.0, -1.0),
    // ];
    // let uvs = vec![
    //     // Right
    //     Vec2::new(1.0, 2.0 / 3.0),
    //     Vec2::new(1.0, 1.0 / 3.0),
    //     Vec2::new(0.75, 1.0 / 3.0),
    //     Vec2::new(0.75, 1.0 / 3.0),
    //     Vec2::new(0.75, 2.0 / 3.0),
    //     Vec2::new(1.0, 2.0 / 3.0),
    // ];
    // let colors = vec![
    //     Color::new(255, 0, 0, 255),     // bottom right
    //     Color::new(0, 255, 0, 255),     // bottom left
    //     Color::new(0, 0, 255, 255),     // top right
    //     Color::new(0, 0, 0, 255), // top left
    // ];
    // let cpu_mesh = CpuMesh {
    //     positions: Positions::F32(positions),
    //     colors: Some(colors),
    //     ..Default::default()
    // };
    let indices = vec![0u8, 1, 2, 2, 3, 0];
    let positions = vec![
        Vec3::new(-1.0, -1.0, 0.0),
        Vec3::new(1.0, -1.0, 0.0),
        Vec3::new(1.0, 1.0, 0.0),
        Vec3::new(-1.0, 1.0, 0.0),
    ];
    let normals = vec![
        Vec3::new(0.0, 0.0, 1.0),
        Vec3::new(0.0, 0.0, 1.0),
        Vec3::new(0.0, 0.0, 1.0),
        Vec3::new(0.0, 0.0, 1.0),
    ];
    let tangents = vec![
        Vec4::new(1.0, 0.0, 0.0, 1.0),
        Vec4::new(1.0, 0.0, 0.0, 1.0),
        Vec4::new(1.0, 0.0, 0.0, 1.0),
        Vec4::new(1.0, 0.0, 0.0, 1.0),
    ];
    let uvs = vec![
        Vec2::new(0.0, 1.0),
        Vec2::new(1.0, 1.0),
        Vec2::new(1.0, 0.0),
        Vec2::new(0.0, 0.0),
    ];
    let colors = vec![
        Color::new(255, 0, 0, 255), // bottom right
        Color::new(0, 255, 0, 255), // bottom left
        Color::new(0, 0, 255, 255), // top right
        Color::new(0, 0, 0, 255),   // top left
    ];
    let cpu_mesh = CpuMesh {
        indices: Indices::U8(indices),
        positions: Positions::F32(positions),
        normals: Some(normals),
        tangents: Some(tangents),
        uvs: Some(uvs),
        colors: Some(colors),
        ..Default::default()
    };
    let toto = blockee::create_block(&context, &mut loaded, vec3(0.0, 0.0, 0.0));

    // Initial properties of the example, 2 cubes per side and non instanced.
    let mut side_count = 4;
    let mut is_instanced = false;

    let axes = Axes::new(&context, 0.1, 30.0);

    let mut gui = three_d::GUI::new(&context);
    window.render_loop(move |mut frame_input| {
        // Gui panel to control the number of cubes and whether or not instancing is turned on.
        let mut panel_width = 0.0;
        gui.update(
            &mut frame_input.events,
            frame_input.accumulated_time,
            frame_input.viewport,
            frame_input.device_pixel_ratio,
            |gui_context| {
                use three_d::egui::*;
                SidePanel::left("side_panel").show(gui_context, |ui| {
                    use three_d::egui::*;
                    ui.heading("Debug Panel");
                    ui.add(Slider::new(&mut side_count, 1..=100).text("Nbr cubes"));
                    ui.add(Checkbox::new(&mut is_instanced, "Use Instancing"));
                });
                panel_width = gui_context.used_rect().width();
            },
        );
        let viewport = Viewport {
            x: (panel_width as f64 * frame_input.device_pixel_ratio) as i32,
            y: 0,
            width: frame_input.viewport.width
                - (panel_width as f64 * frame_input.device_pixel_ratio) as u32,
            height: frame_input.viewport.height,
        };
        camera.set_viewport(viewport);

        // Camera control must be after the gui update.
        control.handle_events(&mut camera, &mut frame_input.events);

        // Ensure we have the correct number of cubes, does no work if already correctly sized.
        let count = side_count * side_count * side_count;
        if non_instanced_meshes.len() != count {
            non_instanced_meshes.clear();
            for i in 0..count {
                let mut gm = Gm::new(
                    // Mesh::new(&context, &CpuMesh::cube()),
                    // PhysicalMaterial::new(
                    //     &context,
                    //     &CpuMaterial {
                    //         albedo: Color {
                    //             r: 128,
                    //             g: 128,
                    //             b: 128,
                    //             a: 255,
                    //         },
                    //         ..Default::default()
                    //     },
                    // ),
                    // !
                    Mesh::new(&context, &cpu_mesh),
                    ColorMaterial::default(),
                    // !
                    // Rectangle::new(&context, vec2(200.0, 200.0), degrees(45.0), 100.0, 100.0),
                    // ColorMaterial::default(),
                );
                let x = (i % side_count) as f32;
                let y = ((i as f32 / side_count as f32).floor() as usize % side_count) as f32;
                let z = (i as f32 / side_count.pow(2) as f32).floor();
                gm.set_transformation(Mat4::from_translation(
                    3.0 * vec3(x, y, z) - 1.5 * (side_count as f32) * vec3(1.0, 1.0, 1.0),
                ));
                // ! gm.set_animation(|time| Mat4::from_angle_x(Rad(time)));
                non_instanced_meshes.push(gm);
            }
        }

        if instanced_mesh.instance_count() != count as u32 {
            instanced_mesh.set_instances(&Instances {
                transformations: (0..count)
                    .map(|i| {
                        let x = (i % side_count) as f32;
                        let y =
                            ((i as f32 / side_count as f32).floor() as usize % side_count) as f32;
                        let z = (i as f32 / side_count.pow(2) as f32).floor();
                        Mat4::from_translation(
                            2.0 * vec3(x, y, z) - 0.0 * (side_count as f32) * vec3(1.0, 1.0, 1.0),
                        )
                    })
                    .collect(),
                ..Default::default()
            });
        }

        // Always update the transforms for both the normal cubes as well as the instanced versions.
        // This shows that the difference in frame rate is not because of updating the transforms
        // and shows that the performance difference is not related to how we update the cubes.
        let time = (frame_input.accumulated_time * 0.001) as f32;
        // instanced_mesh.animate(time);
        non_instanced_meshes
            .iter_mut()
            .for_each(|m| m.animate(time));

        for event in frame_input.events.iter_mut() {
                // println!("input {:?}", frame_input.events);
                if let Event::KeyPress {
                    kind,
                    handled,
                    modifiers,
                    ..
                } = event
                {
                    if *kind == Key::A {
                        let mut aa = camera.view_direction() * 1 as f32 * 1.0;
                        aa.x = 0.0;
                        aa.y = 1.0;
                        aa.z = 0.0;
                        println!("A = {:?}", aa);
                        camera.translate(&aa);
                    }
                    if *kind == Key::E {
                        let mut aa = -camera.view_direction() * 1 as f32 * 1.0;
                        aa.x = 0.0;
                        aa.y = -1.0;
                        aa.z = 0.0;
                        println!("E = {:?}", aa);
                        camera.translate(&aa);
                    }
                    ////////!
                    if *kind == Key::Z {
                        let mut aa = camera.view_direction() * 1 as f32 * 1.0;
                        aa.y = 0.0;
                        println!("Z = {:?}", aa);
                        camera.translate(&aa);
                    }
                    if *kind == Key::S {
                        let mut aa = -camera.view_direction() * 1 as f32 * 1.0;
                        aa.y = 0.0;
                        println!("S = {:?}", aa);
                        camera.translate(&aa);
                    }
                    if *kind == Key::Q {
                        let change = -camera.right_direction() * 1 as f32 * 1.0;
                        println!("Q = {:?}", change);
                        camera.translate(&change);
                    }
                    if *kind == Key::D {
                        let change = camera.right_direction() * 1 as f32 * 1.0;
                        println!("D = {:?}", change);
                        camera.translate(&change);
                    }
                    ////////!
                    if *kind == Key::O {
                        let mut tt = *camera.target();
                        tt.y += 5.0;
                        // tt.x = 0.0;
                        // tt.z = 0.0;
                        camera.set_view(*camera.position(), tt, *camera.up());
                        println!("=========================");
                        println!("O up = {:?}", camera.target());
                    }
                    if *kind == Key::L {
                        let mut tt = *camera.target();
                        tt.y -= 5.0;
                        camera.set_view(*camera.position(), tt, *camera.up());
                        println!("=========================");
                        println!("L up = {:?}", camera.target());
                    }
                    if *kind == Key::K {
                        let mut tt = *camera.target();
                        // tt.x -= 5.0;
                        tt.z += 5.0;
                        camera.set_view(*camera.position(), tt, *camera.up());
                        println!("=========================");
                        println!("K up = {:?}", camera.target());
                    }
                    if *kind == Key::M {
                        let mut tt = *camera.target();
                        // tt.x += 5.0;
                        tt.z -= 5.0;
                        camera.set_view(*camera.position(), tt, *camera.up());
                        println!("=========================");
                        println!("M up = {:?}", camera.target());
                    }
                    ////////!
                    if *kind == Key::W {
                        camera.pitch(radians(1.0 as f32));
                        println!("=========================");
                        println!("W up = {:?}", camera.up());
                    }
                    if *kind == Key::X {
                        camera.pitch(radians(-1.0 as f32));
                        println!("=========================");
                        println!("X up = {:?}", camera.up());
                    }
                }
                if let Event::MouseMotion {
                    button,
                    delta,
                    position,
                    modifiers,
                    handled
                } = event
                {

                    // println!("=========================");
                    delta.0 *= 0.005;
                    delta.1 *= 0.005;
                    camera.yaw(radians(-delta.0 as f32));
                    camera.pitch(radians(-delta.1 as f32));
                    println!("delta = {:?}", delta);
                }
                // if let Event::MousePress {
                //     button,
                //     position,
                //     modifiers,
                //     ..
                // } = event
                // {
                // //     if *button == Event::KeyPress { kind: Key::Z } MouseButton::Left && !modifiers.ctrl {
                // //         let ep = line.end_point0();
                // //         line.set_endpoints(ep, position);
                // //     }
                // // }
            }

        // Then, based on whether or not we render the instanced cubes, collect the renderable
        // objects.
        let render_objects: Vec<&dyn Object> = if is_instanced {
            instanced_mesh.into_iter().chain(&axes).collect()
        } else {
            toto.geo_mat.into_iter().chain(&axes).collect()
            // non_instanced_meshes
            //     .iter()
            //     .map(|x| x as &dyn Object)
            //     .collect()
        };

        frame_input
            .screen()
            .clear(ClearState::color_and_depth(0.8, 0.8, 0.8, 1.0, 1.0))
            .render(&camera, render_objects, &[&light0, &light1])
            .write(|| gui.render());

        FrameOutput::default()
    });
}
