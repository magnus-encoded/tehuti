//! Builds the release wallpaper archive without launching the compositor.

fn main() -> Result<(), Box<dyn core::error::Error>> {
    let mut arguments = std::env::args_os().skip(1);
    let folder = arguments
        .next()
        .ok_or("usage: pack-wallpapers <folder> <destination>")?;
    let destination = arguments
        .next()
        .ok_or("usage: pack-wallpapers <folder> <destination>")?;
    if arguments.next().is_some() {
        return Err("too many arguments".into());
    }
    tehuti::wallpaper_pack::build(std::path::Path::new(&folder), std::path::Path::new(&destination))?;
    Ok(())
}
