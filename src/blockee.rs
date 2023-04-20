use three_d::*;
use three_d_asset::io::RawAssets;

pub struct Block {
    pub geo_mat: Gm<InstancedMesh, ColorMaterial>,
    pub visible: bool,
    pub broken: bool,
    pub brokable: bool,
}

pub fn create_block(
    context: &Context,
    block_texture: &mut RawAssets,
    position: Vec3,
) -> Block {
    let mut geeem: Gm<InstancedMesh, ColorMaterial> = Gm::new(
        InstancedMesh::new(&context, &Instances::default(), &CpuMesh::cube()),
        ColorMaterial {
            texture: Some(
                std::sync::Arc::new(Texture2D::new(
                    &context,
                    &block_texture.deserialize("block").unwrap(),
                ))
                .into(),
            ),
            ..Default::default()
        },
    );

    // geeem.set_transformation(Mat4::from_translation(
    //     3.0 * vec3(1.0 as f32, 2.0 as f32, 3.0 as f32) - 1.5 * (4.0 as f32) * vec3(1.0, 1.0, 1.0),
    // ));
    geeem.set_instances(&Instances {
        transformations: (0.. 4).map(|i| {
            Mat4::from_translation(
                vec3(1.0 * i as f32, 1.0 * i as f32, 1.0 * i as f32),
            )
        }).collect(),
        ..Default::default()
    });
    // geeem.set_instances(&Instances {
    //     transformations:
    //             // let x = (i % side_count) as f32;
    //             // let y =
    //             //     ((i as f32 / side_count as f32).floor() as usize % side_count) as f32;
    //             // let z = (i as f32 / side_count.pow(2) as f32).floor();
    //             // Mat4::from_translation(
    //             //     3.0 * vec3(x, y, z) - 1.5 * (side_count as f32) * vec3(1.0, 1.0, 1.0),
    //             // ),
    //     ..Default::default()
    // });

    // geeem.set_transformation(Mat4::from_translation(position));
    return Block {
        geo_mat: geeem,
        visible: true,
        broken: false,
        brokable: true,
    }
}
