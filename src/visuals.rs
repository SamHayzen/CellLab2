use eframe::egui::{self, ColorImage, TextureHandle, TextureOptions};
use png::{ColorType, Decoder, Transformations};
use std::{io::Cursor, path::Path};

pub struct Icons {
    pub tools: [TextureHandle; 5],
    pub plate: [TextureHandle; 4],
}

impl Icons {
    pub fn load(ctx: &egui::Context) -> Self {
        Self {
            tools: [
                icon(
                    ctx,
                    "tool_cell",
                    include_bytes!("../assets/ui/tool_cell.png"),
                ),
                icon(
                    ctx,
                    "tool_move",
                    include_bytes!("../assets/ui/tool_move.png"),
                ),
                icon(ctx, "tool_add", include_bytes!("../assets/ui/tool_add.png")),
                icon(
                    ctx,
                    "tool_remove",
                    include_bytes!("../assets/ui/tool_remove.png"),
                ),
                icon(
                    ctx,
                    "tool_info",
                    include_bytes!("../assets/ui/tool_info.png"),
                ),
            ],
            plate: [
                icon(ctx, "plate", include_bytes!("../assets/ui/plate.png")),
                icon(
                    ctx,
                    "plate_load",
                    include_bytes!("../assets/ui/plate_load.png"),
                ),
                icon(
                    ctx,
                    "plate_save",
                    include_bytes!("../assets/ui/plate_save.png"),
                ),
                icon(
                    ctx,
                    "plate_clear",
                    include_bytes!("../assets/ui/plate_clear.png"),
                ),
            ],
        }
    }
}

fn icon(ctx: &egui::Context, name: &str, bytes: &[u8]) -> TextureHandle {
    ctx.load_texture(
        name,
        decode_png(bytes).expect("bundled APK icon"),
        TextureOptions::LINEAR,
    )
}

pub(crate) fn decode_png(bytes: &[u8]) -> Result<ColorImage, String> {
    let mut decoder = Decoder::new(Cursor::new(bytes));
    decoder.set_transformations(Transformations::EXPAND | Transformations::STRIP_16);
    let mut reader = decoder.read_info().map_err(|error| error.to_string())?;
    let mut buffer = vec![0; reader.output_buffer_size().ok_or("PNG too large")?];
    let frame = reader
        .next_frame(&mut buffer)
        .map_err(|error| error.to_string())?;
    let pixels = &buffer[..frame.buffer_size()];
    let mut rgba = Vec::with_capacity(frame.width as usize * frame.height as usize * 4);
    match frame.color_type {
        ColorType::Rgba => rgba.extend_from_slice(pixels),
        ColorType::Rgb => {
            for pixel in pixels.chunks_exact(3) {
                rgba.extend_from_slice(&[pixel[0], pixel[1], pixel[2], 255]);
            }
        }
        ColorType::GrayscaleAlpha => {
            for pixel in pixels.chunks_exact(2) {
                rgba.extend_from_slice(&[pixel[0], pixel[0], pixel[0], pixel[1]]);
            }
        }
        ColorType::Grayscale => {
            for gray in pixels {
                rgba.extend_from_slice(&[*gray, *gray, *gray, 255]);
            }
        }
        _ => return Err("Unsupported PNG color type".into()),
    }
    Ok(ColorImage::from_rgba_unmultiplied(
        [frame.width as usize, frame.height as usize],
        &rgba,
    ))
}

pub fn save_png(path: &Path, image: &ColorImage) -> Result<(), String> {
    let file = std::fs::File::create(path).map_err(|error| error.to_string())?;
    let mut encoder = png::Encoder::new(file, image.size[0] as u32, image.size[1] as u32);
    encoder.set_color(ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().map_err(|error| error.to_string())?;
    let rgba: Vec<u8> = image
        .pixels
        .iter()
        .flat_map(|pixel| pixel.to_array())
        .collect();
    writer
        .write_image_data(&rgba)
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apk_toolbar_icons_decode_with_transparency() {
        for bytes in [
            include_bytes!("../assets/ui/tool_cell.png").as_slice(),
            include_bytes!("../assets/ui/tool_move.png").as_slice(),
            include_bytes!("../assets/ui/tool_add.png").as_slice(),
            include_bytes!("../assets/ui/tool_remove.png").as_slice(),
            include_bytes!("../assets/ui/tool_info.png").as_slice(),
        ] {
            let image = decode_png(bytes).unwrap();
            assert_eq!(image.size, [96, 96]);
        }
    }
}
