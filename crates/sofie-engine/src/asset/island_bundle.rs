/// The area's bundle the app's build script wrote to `OUT_DIR/islands`, with its compressed copies when built with them.
#[macro_export]
macro_rules! island_bundle {
    ($area:literal) => {{
        let bundle = $crate::islands::IslandBundle::new(
            $area,
            include_bytes!(concat!(env!("OUT_DIR"), "/islands/", $area, ".js")),
            include_bytes!(concat!(env!("OUT_DIR"), "/islands/", $area, ".css")),
        );
        $crate::islands::IslandBundle {
            script: $crate::compressed_asset!(
                bundle.script,
                concat!(env!("OUT_DIR"), "/islands/", $area, ".js")
            ),
            stylesheet: $crate::compressed_asset!(
                bundle.stylesheet,
                concat!(env!("OUT_DIR"), "/islands/", $area, ".css")
            ),
            ..bundle
        }
    }};
}
