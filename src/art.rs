use image::{ImageFormat, ImageReader};
use std::{io::Cursor, path::Path, sync::Arc};
use symphonia::core::meta::{StandardVisualKey, Visual};

const MAX_ENCODED: usize = 4 * 1024 * 1024;
const MAX_DIMENSION: u32 = 4096;
const THUMBNAIL_SIZE: u32 = 256;

#[derive(Clone)]
pub struct CoverArt {
    pub encoded: Arc<Vec<u8>>,
    pub pixels: Arc<Vec<u8>>,
    pub width: u32,
    pub height: u32,
}

pub fn decode(bytes: Vec<u8>) -> Option<Arc<CoverArt>> {
    if bytes.is_empty() || bytes.len() > MAX_ENCODED {
        return None;
    }
    let format = image::guess_format(&bytes).ok()?;
    if !matches!(format, ImageFormat::Jpeg | ImageFormat::Png) {
        return None;
    }
    let dimensions = ImageReader::with_format(Cursor::new(bytes.as_slice()), format)
        .into_dimensions()
        .ok()?;
    if dimensions.0 == 0
        || dimensions.1 == 0
        || dimensions.0 > MAX_DIMENSION
        || dimensions.1 > MAX_DIMENSION
    {
        return None;
    }
    let image = ImageReader::with_format(Cursor::new(bytes.as_slice()), format)
        .decode()
        .ok()?
        .thumbnail(
            dimensions.0.min(THUMBNAIL_SIZE),
            dimensions.1.min(THUMBNAIL_SIZE),
        )
        .to_rgba8();
    let (width, height) = image.dimensions();
    Some(Arc::new(CoverArt {
        encoded: Arc::new(bytes),
        pixels: Arc::new(image.into_raw()),
        width,
        height,
    }))
}

pub fn from_visuals(visuals: &[Visual]) -> Option<Arc<CoverArt>> {
    let mut ordered: Vec<_> = visuals.iter().collect();
    ordered.sort_by_key(|visual| visual.usage != Some(StandardVisualKey::FrontCover));
    ordered
        .into_iter()
        .filter(|visual| visual.data.len() <= MAX_ENCODED)
        .find_map(|visual| decode(visual.data.to_vec()))
}
pub fn from_sidecar(audio_path: &Path) -> Option<Arc<CoverArt>> {
    let directory = audio_path.parent()?;
    let stem = audio_path.file_stem()?.to_str()?;
    let candidates = [
        format!("{stem}.jpg"),
        format!("{stem}.png"),
        "cover.jpg".into(),
        "cover.png".into(),
        "folder.jpg".into(),
        "folder.png".into(),
        "front.jpg".into(),
        "front.png".into(),
    ];
    candidates.into_iter().find_map(|name| {
        let path = directory.join(name);
        let size = std::fs::metadata(&path).ok()?.len();
        if size > MAX_ENCODED as u64 {
            return None;
        }
        decode(std::fs::read(path).ok()?)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_small_png_and_rejects_non_images() {
        let pixels = image::RgbaImage::from_pixel(2, 1, image::Rgba([12, 34, 56, 255]));
        let mut bytes = Vec::new();
        image::DynamicImage::ImageRgba8(pixels)
            .write_to(&mut Cursor::new(&mut bytes), ImageFormat::Png)
            .unwrap();
        let art = decode(bytes).unwrap();
        assert_eq!((art.width, art.height), (2, 1));
        assert_eq!(&art.pixels[..4], &[12, 34, 56, 255]);
        assert!(decode(vec![1, 2, 3]).is_none());
    }
    #[test]
    fn finds_folder_cover_when_embedded_art_is_missing() {
        let directory = std::env::temp_dir().join(format!("cover-test-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let cover_path = directory.join("folder.png");
        image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            3,
            2,
            image::Rgba([90, 80, 70, 255]),
        ))
        .save(&cover_path)
        .unwrap();
        let cover = from_sidecar(&directory.join("song.wav")).unwrap();
        assert_eq!((cover.width, cover.height), (3, 2));
        std::fs::remove_dir_all(directory).unwrap();
    }
}
