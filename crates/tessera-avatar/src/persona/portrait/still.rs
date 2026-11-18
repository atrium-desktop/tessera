use std::path::Path;

use flux::{Device, Format, Image};

use super::{Error, mask::circle_mask_premultiplied};

const PORTRAIT_SIZE: u32 = super::vrm::ATLAS_SIZE;

pub fn build(device: &Device, path: &Path) -> Result<Image, Error> {
    let decoded = tessera_image::decode_file(path)
        .map_err(|error| Error::Decode(path.to_path_buf(), error))?;
    let fitted = tessera_image::resize_to_fill(
        &decoded,
        PORTRAIT_SIZE,
        PORTRAIT_SIZE,
        tessera_image::Filter::Lanczos3,
    );
    let masked = circle_mask_premultiplied(&fitted);
    Image::from_bytes(
        device,
        masked.width(),
        masked.height(),
        Format::Rgba8Unorm,
        masked.as_raw(),
    )
    .map_err(Error::Flux)
}

#[cfg(test)]
mod tests {
    #[test]
    fn cover_fit_squares_any_aspect() {
        let landscape =
            tessera_image::RgbaImage::from_raw(400, 200, vec![255; 400 * 200 * 4]).unwrap();
        let fitted =
            tessera_image::resize_to_fill(&landscape, 32, 32, tessera_image::Filter::Nearest);
        assert_eq!(fitted.dimensions(), (32, 32));
    }
}
