use optics_gfx::{AddressMode, Device, Filter, Format, Image, Sampler, SamplerDesc};

use crate::render::{
    Material, MaterialAlphaMode, MaterialDesc, MaterialKind, MaterialOptions, MaterialTexture,
};

use crate::glb_json::{AlphaMode, GlbDocument, GlbImage, GlbSampler};
use crate::{DecodedImage, LoadError, MaterialTarget};

pub(crate) const MAX_TEXTURE_EDGE: u32 = 8192;
pub(crate) const MAX_IMAGE_ALLOCATION: u64 = 256 * 1024 * 1024;
const MAX_TOTAL_TEXTURE_BYTES: u64 = 512 * 1024 * 1024;

pub(crate) fn load<F>(
    device: &Device,
    doc: &GlbDocument<'_>,
    target: MaterialTarget,
    mut decode: F,
) -> Result<(Vec<Material>, Material), LoadError>
where
    F: FnMut(&str, &[u8]) -> Result<DecodedImage, String>,
{
    let mut required_textures = vec![false; doc.textures.len()];
    for material in &doc.materials {
        if let Some(info) = &material.base_color_texture {
            let tex = doc.textures.get(info.index).ok_or_else(|| {
                LoadError::Gltf(format!("texture index {} out of bounds", info.index))
            })?;
            let _ = tex;
            required_textures[info.index] = true;
        }
    }

    let mut images: Vec<Option<Image>> = std::iter::repeat_with(|| None)
        .take(doc.images.len())
        .collect();
    let mut samplers: Vec<Option<Sampler>> = std::iter::repeat_with(|| None)
        .take(doc.samplers.len())
        .collect();
    let mut default_sampler = None;
    let mut total_texture_bytes = 0u64;

    for (tex_index, texture) in doc.textures.iter().enumerate() {
        if !required_textures[tex_index] {
            continue;
        }
        let image_index = texture.source.ok_or_else(|| {
            LoadError::Unsupported(format!("texture {tex_index} has no source image"))
        })?;
        let image = doc
            .images
            .get(image_index)
            .ok_or_else(|| LoadError::Gltf(format!("image index {image_index} out of bounds")))?;
        if images[image_index].is_none() {
            let decoded = decode_image(device, doc, image, &mut decode)?;
            let (width, height) = decoded.size();
            let bytes = u64::from(width)
                .checked_mul(u64::from(height))
                .and_then(|pixels| pixels.checked_mul(4))
                .ok_or_else(|| LoadError::Unsupported("decoded image size overflows".into()))?;
            total_texture_bytes = total_texture_bytes
                .checked_add(bytes)
                .filter(|total| *total <= MAX_TOTAL_TEXTURE_BYTES)
                .ok_or_else(|| {
                    LoadError::Unsupported("decoded base-colour textures exceed 512 MiB".into())
                })?;
            images[image_index] = Some(decoded);
        }
        if let Some(sampler_index) = texture.sampler {
            let sampler = doc.samplers.get(sampler_index).ok_or_else(|| {
                LoadError::Gltf(format!("sampler index {sampler_index} out of bounds"))
            })?;
            if samplers[sampler_index].is_none() {
                samplers[sampler_index] = Some(Sampler::new(device, sampler_desc(sampler))?);
            }
        } else if default_sampler.is_none() {
            default_sampler = Some(Sampler::new(device, sampler_desc(&GlbSampler::default()))?);
        }
    }

    let mut materials = Vec::with_capacity(doc.materials.len());
    for (mat_index, material) in doc.materials.iter().enumerate() {
        let texture = if let Some(info) = &material.base_color_texture {
            let mut tex_coord = info.tex_coord;
            let mut uv_offset = [0.0, 0.0];
            let mut uv_scale = [1.0, 1.0];
            let mut uv_rotation = 0.0;
            if let Some(transform) = &info.transform {
                tex_coord = transform.tex_coord.unwrap_or(tex_coord);
                uv_offset = transform.offset;
                uv_scale = transform.scale;
                uv_rotation = transform.rotation;
            }
            if tex_coord != 0 {
                return Err(LoadError::Unsupported(format!(
                    "material {mat_index} uses TEXCOORD_{tex_coord}; only TEXCOORD_0 is uploaded"
                )));
            }
            let tex = &doc.textures[info.index];
            let image_index = tex
                .source
                .ok_or_else(|| LoadError::Unsupported("base-colour image was not loaded".into()))?;
            let image = images[image_index]
                .as_ref()
                .ok_or_else(|| LoadError::Unsupported("base-colour image was not loaded".into()))?;
            let sampler = match tex.sampler {
                Some(index) => samplers[index].as_ref(),
                None => default_sampler.as_ref(),
            };
            Some(MaterialTexture {
                image,
                sampler,
                uv_offset,
                uv_scale,
                uv_rotation,
            })
        } else {
            None
        };

        let alpha_mode = match material.alpha_mode {
            AlphaMode::Opaque => MaterialAlphaMode::Opaque,
            AlphaMode::Mask => MaterialAlphaMode::Mask,
            AlphaMode::Blend => MaterialAlphaMode::Blend,
        };
        materials.push(Material::new_with_options(
            device,
            MaterialDesc {
                kind: if material.unlit {
                    MaterialKind::Unlit
                } else {
                    MaterialKind::Phong
                },
                base_color: material.base_color_factor,
                color_format: target.color_format,
                depth_format: target.depth_format,
                shininess: 32.0,
                specular: 0.0,
            },
            MaterialOptions {
                base_color_texture: texture,
                alpha_mode,
                alpha_cutoff: material.alpha_cutoff,
                double_sided: material.double_sided,
            },
        )?);
    }

    let fallback = Material::new(
        device,
        MaterialDesc {
            kind: MaterialKind::Phong,
            base_color: [1.0; 4],
            color_format: target.color_format,
            depth_format: target.depth_format,
            shininess: 32.0,
            specular: 0.0,
        },
    )?;
    Ok((materials, fallback))
}

fn validate_decoded_image(decoded: &DecodedImage) -> Result<(), LoadError> {
    if decoded.width == 0
        || decoded.height == 0
        || decoded.width > MAX_TEXTURE_EDGE
        || decoded.height > MAX_TEXTURE_EDGE
    {
        return Err(LoadError::Image(format!(
            "decoded image dimensions {}x{} exceed limit {MAX_TEXTURE_EDGE}",
            decoded.width, decoded.height
        )));
    }
    let expected = u64::from(decoded.width)
        .checked_mul(u64::from(decoded.height))
        .and_then(|pixels| pixels.checked_mul(4))
        .filter(|len| *len <= MAX_IMAGE_ALLOCATION)
        .ok_or_else(|| {
            LoadError::Image("decoded image buffer exceeds allocation limit".to_string())
        })?;
    if decoded.rgba.len() as u64 != expected {
        return Err(LoadError::Image(format!(
            "decoded image RGBA length {} does not match {}x{} ({expected} bytes)",
            decoded.rgba.len(),
            decoded.width,
            decoded.height
        )));
    }
    Ok(())
}

fn decode_image<F>(
    device: &Device,
    doc: &GlbDocument<'_>,
    image: &GlbImage,
    decode: &mut F,
) -> Result<Image, LoadError>
where
    F: FnMut(&str, &[u8]) -> Result<DecodedImage, String>,
{
    if image.uri.is_some() {
        return Err(LoadError::Unsupported(
            "external image URIs are unsupported for in-memory GLB loading".into(),
        ));
    }
    let view_index = image
        .buffer_view
        .ok_or_else(|| LoadError::Unsupported("image has neither bufferView nor URI".into()))?;
    let view = doc
        .buffer_views
        .get(view_index)
        .ok_or_else(|| LoadError::Gltf(format!("bufferView index {view_index} out of bounds")))?;
    if view.buffer != 0 {
        return Err(LoadError::Unsupported(
            "base-colour image references an external buffer".into(),
        ));
    }
    let blob = doc
        .bin_chunk
        .ok_or_else(|| LoadError::Unsupported("GLB has no BIN chunk".into()))?;
    let end = view
        .byte_offset
        .checked_add(view.byte_length)
        .ok_or_else(|| LoadError::Unsupported("image buffer view overflows".into()))?;
    let encoded = blob
        .get(view.byte_offset..end)
        .ok_or_else(|| LoadError::Unsupported("image buffer view is out of bounds".into()))?;

    let mime = image
        .mime_type
        .as_deref()
        .ok_or_else(|| LoadError::Unsupported("image bufferView missing mimeType".into()))?;
    let decoded = decode(mime, encoded).map_err(LoadError::Image)?;
    validate_decoded_image(&decoded)?;

    Image::from_bytes(
        device,
        decoded.width,
        decoded.height,
        Format::Rgba8Srgb,
        &decoded.rgba,
    )
    .map_err(LoadError::from)
}

fn sampler_desc(sampler: &GlbSampler) -> SamplerDesc {
    let (min_filter, mipmap_mode) = match sampler.min_filter {
        None | Some(9729) => (Filter::Linear, Filter::Linear),
        Some(9728) => (Filter::Nearest, Filter::Nearest),
        Some(9984) => (Filter::Nearest, Filter::Nearest),
        Some(9985) => (Filter::Linear, Filter::Nearest),
        Some(9986) => (Filter::Nearest, Filter::Linear),
        Some(9987) => (Filter::Linear, Filter::Linear),
        Some(_) => (Filter::Linear, Filter::Linear),
    };
    SamplerDesc {
        min_filter,
        mag_filter: match sampler.mag_filter {
            Some(9728) => Filter::Nearest,
            None | Some(9729) | Some(_) => Filter::Linear,
        },
        mipmap_mode,
        address_u: address_mode(sampler.wrap_s),
        address_v: address_mode(sampler.wrap_t),
        address_w: AddressMode::Repeat,
        max_anisotropy: 1.0,
    }
}

fn address_mode(mode: u32) -> AddressMode {
    match mode {
        33071 => AddressMode::ClampToEdge,
        33648 => AddressMode::MirroredRepeat,
        _ => AddressMode::Repeat,
    }
}

#[cfg(test)]
mod tests {
    use super::{MAX_TEXTURE_EDGE, validate_decoded_image};
    use crate::{DecodedImage, LoadError};

    #[test]
    fn accepts_valid_rgba_image() {
        let img = DecodedImage {
            width: 2,
            height: 2,
            rgba: vec![255; 16],
        };
        assert!(validate_decoded_image(&img).is_ok());
    }

    #[test]
    fn rejects_zero_or_oversized_dimensions() {
        let zero = DecodedImage {
            width: 0,
            height: 4,
            rgba: Vec::new(),
        };
        assert!(matches!(
            validate_decoded_image(&zero),
            Err(LoadError::Image(_))
        ));

        let huge = DecodedImage {
            width: MAX_TEXTURE_EDGE + 1,
            height: 1,
            rgba: Vec::new(),
        };
        assert!(matches!(
            validate_decoded_image(&huge),
            Err(LoadError::Image(_))
        ));
    }

    #[test]
    fn rejects_mismatched_buffer_length() {
        let short = DecodedImage {
            width: 2,
            height: 2,
            rgba: vec![0; 12],
        };
        assert!(matches!(
            validate_decoded_image(&short),
            Err(LoadError::Image(_))
        ));
    }
}
