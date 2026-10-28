//! Render the reusable choice list fixture without a display or project scan.
use tehuti_core::{
    choice_list::{Candidate, Create, List, Slots},
    pad::PadFamily,
};

fn main() -> Result<(), Box<dyn core::error::Error>> {
    let path = std::env::args_os().nth(1).ok_or("provide an output PNG path")?;
    let list = List::new(Slots {
        title: "Choose a workspace".into(),
        heard: "Orbyt".into(),
        scopes: vec!["Examples: 8 workspaces".into()],
        create: Some(Create {
            heading: "+ NEW WORKSPACE".into(),
            detail: "Use the current draft".into(),
            action: "Create".into(),
            enabled: true,
        }),
        candidates: [
            "Orbit", "Orbits", "Atlas", "Beacon", "Cedar", "Delta", "Ember", "Fern",
        ]
        .into_iter()
        .map(|name| Candidate {
            name: name.into(),
            meta: Some("Example workspace".into()),
        })
        .collect(),
        action: "Open".into(),
        closest_heading: "CLOSEST".into(),
        remaining_heading: "ALL WORKSPACES".into(),
    });
    let pixels =
        tehuti::choice_list::raster(&list, PadFamily::Letters, (1920, 1080)).ok_or("artwork unavailable")?;
    let mut image = image::RgbaImage::from_raw(1920, 1080, pixels).ok_or("invalid raster size")?;
    for pixel in image.pixels_mut() {
        pixel.0.swap(0, 2);
    }
    image.save(path)?;
    Ok(())
}
