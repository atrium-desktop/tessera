//! Minimal GLB 2.0 container & JSON parser for material/texture extraction.

use std::collections::BTreeMap;

const GLB_MAGIC: u32 = 0x4654_6C67; // "glTF"
const GLB_VERSION_2: u32 = 2;
const CHUNK_JSON: u32 = 0x4E4F_534A; // "JSON"
const CHUNK_BIN: u32 = 0x004E_4942; // "BIN\0"
const MAX_JSON_DEPTH: usize = 64;

pub(crate) struct GlbDocument<'a> {
    pub bin_chunk: Option<&'a [u8]>,
    pub buffer_views: Vec<BufferView>,
    pub images: Vec<GlbImage>,
    pub samplers: Vec<GlbSampler>,
    pub textures: Vec<GlbTexture>,
    pub materials: Vec<GlbMaterial>,
}

pub(crate) struct BufferView {
    pub buffer: usize,
    pub byte_offset: usize,
    pub byte_length: usize,
}

pub(crate) struct GlbImage {
    pub buffer_view: Option<usize>,
    pub mime_type: Option<String>,
    pub uri: Option<String>,
}

#[derive(Clone, Copy)]
pub(crate) struct GlbSampler {
    pub mag_filter: Option<u32>,
    pub min_filter: Option<u32>,
    pub wrap_s: u32,
    pub wrap_t: u32,
}

impl Default for GlbSampler {
    fn default() -> Self {
        Self {
            mag_filter: None,
            min_filter: None,
            wrap_s: 10497, // REPEAT
            wrap_t: 10497, // REPEAT
        }
    }
}

pub(crate) struct GlbTexture {
    pub source: Option<usize>,
    pub sampler: Option<usize>,
}

pub(crate) struct TextureTransform {
    pub tex_coord: Option<u32>,
    pub offset: [f32; 2],
    pub scale: [f32; 2],
    pub rotation: f32,
}

pub(crate) struct BaseColorTexture {
    pub index: usize,
    pub tex_coord: u32,
    pub transform: Option<TextureTransform>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum AlphaMode {
    Opaque,
    Mask,
    Blend,
}

pub(crate) struct GlbMaterial {
    pub base_color_factor: [f32; 4],
    pub base_color_texture: Option<BaseColorTexture>,
    pub alpha_mode: AlphaMode,
    pub alpha_cutoff: f32,
    pub double_sided: bool,
    pub unlit: bool,
}

/// The little-endian `u32` at `offset`, or `None` past the end of `bytes`.
fn read_u32(bytes: &[u8], offset: usize) -> Option<u32> {
    let end = offset.checked_add(4)?;
    let word: [u8; 4] = bytes.get(offset..end)?.try_into().ok()?;
    Some(u32::from_le_bytes(word))
}

pub(crate) fn parse_glb(bytes: &[u8]) -> Result<GlbDocument<'_>, String> {
    let header =
        |offset| read_u32(bytes, offset).ok_or_else(|| "GLB header is truncated".to_string());
    let magic = header(0)?;
    let version = header(4)?;
    let total_len = header(8)? as usize;
    if magic != GLB_MAGIC {
        return Err("invalid GLB magic".into());
    }
    if version != GLB_VERSION_2 {
        return Err(format!("unsupported GLB version {version}"));
    }
    if total_len < 12 || total_len > bytes.len() {
        return Err("GLB length is out of bounds".into());
    }

    let mut offset = 12usize;
    let mut json_chunk: Option<&[u8]> = None;
    let mut bin_chunk: Option<&[u8]> = None;

    while offset < total_len {
        if offset + 8 > total_len {
            return Err("GLB chunk header is truncated".into());
        }
        let chunk_header =
            |at| read_u32(bytes, at).ok_or_else(|| "GLB chunk header is truncated".to_string());
        let chunk_len = chunk_header(offset)? as usize;
        let chunk_type = chunk_header(offset + 4)?;
        offset += 8;
        let end = offset
            .checked_add(chunk_len)
            .filter(|e| *e <= total_len)
            .ok_or_else(|| "GLB chunk length exceeds container".to_string())?;
        let data = &bytes[offset..end];
        if chunk_type == CHUNK_JSON && json_chunk.is_none() {
            json_chunk = Some(data);
        } else if chunk_type == CHUNK_BIN && bin_chunk.is_none() {
            bin_chunk = Some(data);
        }
        offset = end;
    }

    let json_bytes = json_chunk.ok_or_else(|| "GLB has no JSON chunk".to_string())?;
    let json_str = std::str::from_utf8(json_bytes)
        .map_err(|e| format!("GLB JSON chunk is not valid UTF-8: {e}"))?;
    let root = JsonParser::new(json_str).parse()?;
    let root_obj = root
        .as_object()
        .ok_or_else(|| "glTF root must be a JSON object".to_string())?;

    let buffer_views = parse_buffer_views(root_obj)?;
    let images = parse_images(root_obj)?;
    let samplers = parse_samplers(root_obj)?;
    let textures = parse_textures(root_obj)?;
    let materials = parse_materials(root_obj)?;

    Ok(GlbDocument {
        bin_chunk,
        buffer_views,
        images,
        samplers,
        textures,
        materials,
    })
}

fn parse_buffer_views(root: &BTreeMap<String, JsonValue>) -> Result<Vec<BufferView>, String> {
    let Some(arr) = root.get("bufferViews").and_then(JsonValue::as_array) else {
        return Ok(Vec::new());
    };
    let mut out = Vec::with_capacity(arr.len());
    for item in arr {
        let obj = item
            .as_object()
            .ok_or_else(|| "bufferView must be an object".to_string())?;
        let buffer = obj
            .get("buffer")
            .and_then(JsonValue::as_usize)
            .ok_or_else(|| "bufferView missing buffer index".to_string())?;
        let byte_offset = obj
            .get("byteOffset")
            .and_then(JsonValue::as_usize)
            .unwrap_or(0);
        let byte_length = obj
            .get("byteLength")
            .and_then(JsonValue::as_usize)
            .ok_or_else(|| "bufferView missing byteLength".to_string())?;
        out.push(BufferView {
            buffer,
            byte_offset,
            byte_length,
        });
    }
    Ok(out)
}

fn parse_images(root: &BTreeMap<String, JsonValue>) -> Result<Vec<GlbImage>, String> {
    let Some(arr) = root.get("images").and_then(JsonValue::as_array) else {
        return Ok(Vec::new());
    };
    let mut out = Vec::with_capacity(arr.len());
    for item in arr {
        let obj = item
            .as_object()
            .ok_or_else(|| "image must be an object".to_string())?;
        out.push(GlbImage {
            buffer_view: obj.get("bufferView").and_then(JsonValue::as_usize),
            mime_type: obj
                .get("mimeType")
                .and_then(JsonValue::as_str)
                .map(str::to_owned),
            uri: obj
                .get("uri")
                .and_then(JsonValue::as_str)
                .map(str::to_owned),
        });
    }
    Ok(out)
}

fn parse_samplers(root: &BTreeMap<String, JsonValue>) -> Result<Vec<GlbSampler>, String> {
    let Some(arr) = root.get("samplers").and_then(JsonValue::as_array) else {
        return Ok(Vec::new());
    };
    let mut out = Vec::with_capacity(arr.len());
    for item in arr {
        let obj = item
            .as_object()
            .ok_or_else(|| "sampler must be an object".to_string())?;
        out.push(GlbSampler {
            mag_filter: obj.get("magFilter").and_then(JsonValue::as_u32),
            min_filter: obj.get("minFilter").and_then(JsonValue::as_u32),
            wrap_s: obj
                .get("wrapS")
                .and_then(JsonValue::as_u32)
                .unwrap_or(10497),
            wrap_t: obj
                .get("wrapT")
                .and_then(JsonValue::as_u32)
                .unwrap_or(10497),
        });
    }
    Ok(out)
}

fn parse_textures(root: &BTreeMap<String, JsonValue>) -> Result<Vec<GlbTexture>, String> {
    let Some(arr) = root.get("textures").and_then(JsonValue::as_array) else {
        return Ok(Vec::new());
    };
    let mut out = Vec::with_capacity(arr.len());
    for item in arr {
        let obj = item
            .as_object()
            .ok_or_else(|| "texture must be an object".to_string())?;
        out.push(GlbTexture {
            source: obj.get("source").and_then(JsonValue::as_usize),
            sampler: obj.get("sampler").and_then(JsonValue::as_usize),
        });
    }
    Ok(out)
}

fn parse_materials(root: &BTreeMap<String, JsonValue>) -> Result<Vec<GlbMaterial>, String> {
    let Some(arr) = root.get("materials").and_then(JsonValue::as_array) else {
        return Ok(Vec::new());
    };
    let mut out = Vec::with_capacity(arr.len());
    for item in arr {
        let obj = item
            .as_object()
            .ok_or_else(|| "material must be an object".to_string())?;
        let pbr = obj
            .get("pbrMetallicRoughness")
            .and_then(JsonValue::as_object);
        let base_color_factor = pbr
            .and_then(|p| p.get("baseColorFactor"))
            .and_then(|v| v.as_f32_array::<4>())
            .unwrap_or([1.0, 1.0, 1.0, 1.0]);

        let base_color_texture = if let Some(tex_info) = pbr
            .and_then(|p| p.get("baseColorTexture"))
            .and_then(JsonValue::as_object)
        {
            let index = tex_info
                .get("index")
                .and_then(JsonValue::as_usize)
                .ok_or_else(|| "baseColorTexture missing index".to_string())?;
            let tex_coord = tex_info
                .get("texCoord")
                .and_then(JsonValue::as_u32)
                .unwrap_or(0);
            let transform = tex_info
                .get("extensions")
                .and_then(JsonValue::as_object)
                .and_then(|ext| ext.get("KHR_texture_transform"))
                .and_then(JsonValue::as_object)
                .map(|tr| TextureTransform {
                    tex_coord: tr.get("texCoord").and_then(JsonValue::as_u32),
                    offset: tr
                        .get("offset")
                        .and_then(|v| v.as_f32_array::<2>())
                        .unwrap_or([0.0, 0.0]),
                    scale: tr
                        .get("scale")
                        .and_then(|v| v.as_f32_array::<2>())
                        .unwrap_or([1.0, 1.0]),
                    rotation: tr
                        .get("rotation")
                        .and_then(JsonValue::as_f32)
                        .unwrap_or(0.0),
                });
            Some(BaseColorTexture {
                index,
                tex_coord,
                transform,
            })
        } else {
            None
        };

        let alpha_mode = match obj.get("alphaMode").and_then(JsonValue::as_str) {
            None | Some("OPAQUE") => AlphaMode::Opaque,
            Some("MASK") => AlphaMode::Mask,
            Some("BLEND") => AlphaMode::Blend,
            Some(other) => return Err(format!("unsupported alphaMode {other}")),
        };
        let alpha_cutoff = obj
            .get("alphaCutoff")
            .and_then(JsonValue::as_f32)
            .unwrap_or(0.5);
        let double_sided = obj
            .get("doubleSided")
            .and_then(JsonValue::as_bool)
            .unwrap_or(false);
        let unlit = obj
            .get("extensions")
            .and_then(JsonValue::as_object)
            .is_some_and(|ext| ext.contains_key("KHR_materials_unlit"));

        out.push(GlbMaterial {
            base_color_factor,
            base_color_texture,
            alpha_mode,
            alpha_cutoff,
            double_sided,
            unlit,
        });
    }
    Ok(out)
}

#[derive(Debug)]
enum JsonValue {
    Null,
    Bool(bool),
    Number(f64),
    String(String),
    Array(Vec<JsonValue>),
    Object(BTreeMap<String, JsonValue>),
}

impl JsonValue {
    fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(b) => Some(*b),
            _ => None,
        }
    }

    fn as_f32(&self) -> Option<f32> {
        match self {
            Self::Number(n) => Some(*n as f32),
            _ => None,
        }
    }

    fn as_u32(&self) -> Option<u32> {
        match self {
            Self::Number(n) if *n >= 0.0 && *n <= u32::MAX as f64 => Some(*n as u32),
            _ => None,
        }
    }

    fn as_usize(&self) -> Option<usize> {
        match self {
            Self::Number(n) if *n >= 0.0 && *n <= usize::MAX as f64 => Some(*n as usize),
            _ => None,
        }
    }

    fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(s) => Some(s.as_str()),
            _ => None,
        }
    }

    fn as_array(&self) -> Option<&[JsonValue]> {
        match self {
            Self::Array(arr) => Some(arr),
            _ => None,
        }
    }

    fn as_object(&self) -> Option<&BTreeMap<String, JsonValue>> {
        match self {
            Self::Object(obj) => Some(obj),
            _ => None,
        }
    }

    fn as_f32_array<const N: usize>(&self) -> Option<[f32; N]> {
        let arr = self.as_array()?;
        if arr.len() != N {
            return None;
        }
        let mut out = [0.0f32; N];
        for (dst, src) in out.iter_mut().zip(arr.iter()) {
            *dst = src.as_f32()?;
        }
        Some(out)
    }
}

struct JsonParser<'a> {
    bytes: &'a [u8],
    pos: usize,
    depth: usize,
}

impl<'a> JsonParser<'a> {
    fn new(input: &'a str) -> Self {
        Self {
            bytes: input.as_bytes(),
            pos: 0,
            depth: 0,
        }
    }

    fn parse(&mut self) -> Result<JsonValue, String> {
        self.skip_ws();
        let val = self.parse_value()?;
        self.skip_ws();
        Ok(val)
    }

    fn skip_ws(&mut self) {
        while self.pos < self.bytes.len()
            && matches!(self.bytes[self.pos], b' ' | b'\t' | b'\n' | b'\r')
        {
            self.pos += 1;
        }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn next(&mut self) -> Option<u8> {
        let b = self.bytes.get(self.pos).copied()?;
        self.pos += 1;
        Some(b)
    }

    fn parse_value(&mut self) -> Result<JsonValue, String> {
        self.skip_ws();
        match self.peek() {
            Some(b'n') => self.parse_literal(b"null", JsonValue::Null),
            Some(b't') => self.parse_literal(b"true", JsonValue::Bool(true)),
            Some(b'f') => self.parse_literal(b"false", JsonValue::Bool(false)),
            Some(b'"') => self.parse_string().map(JsonValue::String),
            Some(b'[') => self.parse_array(),
            Some(b'{') => self.parse_object(),
            Some(b'-' | b'0'..=b'9') => self.parse_number(),
            Some(other) => Err(format!(
                "unexpected JSON byte 0x{other:02x} at {}",
                self.pos
            )),
            None => Err("unexpected end of JSON".into()),
        }
    }

    fn parse_literal(&mut self, expected: &[u8], value: JsonValue) -> Result<JsonValue, String> {
        if self.bytes[self.pos..].starts_with(expected) {
            self.pos += expected.len();
            Ok(value)
        } else {
            Err(format!("invalid JSON literal at {}", self.pos))
        }
    }

    fn parse_number(&mut self) -> Result<JsonValue, String> {
        let start = self.pos;
        if self.peek() == Some(b'-') {
            self.pos += 1;
        }
        while self
            .peek()
            .is_some_and(|b| b.is_ascii_digit() || matches!(b, b'.' | b'e' | b'E' | b'+' | b'-'))
        {
            self.pos += 1;
        }
        let s = std::str::from_utf8(&self.bytes[start..self.pos])
            .map_err(|e| format!("invalid UTF-8 in JSON number: {e}"))?;
        let n: f64 = s
            .parse()
            .map_err(|e| format!("invalid JSON number {s:?}: {e}"))?;
        Ok(JsonValue::Number(n))
    }

    fn parse_string(&mut self) -> Result<String, String> {
        if self.next() != Some(b'"') {
            return Err("expected '\"'".into());
        }
        let mut out = String::new();
        loop {
            let b = self
                .next()
                .ok_or_else(|| "unterminated JSON string".to_string())?;
            if b == b'"' {
                return Ok(out);
            }
            if b == b'\\' {
                let esc = self
                    .next()
                    .ok_or_else(|| "unterminated JSON escape".to_string())?;
                match esc {
                    b'"' => out.push('"'),
                    b'\\' => out.push('\\'),
                    b'/' => out.push('/'),
                    b'b' => out.push('\u{0008}'),
                    b'f' => out.push('\u{000C}'),
                    b'n' => out.push('\n'),
                    b'r' => out.push('\r'),
                    b't' => out.push('\t'),
                    b'u' => {
                        let code = self.parse_hex4()?;
                        if (0xD800..=0xDBFF).contains(&code) {
                            if self.next() != Some(b'\\') || self.next() != Some(b'u') {
                                return Err("invalid UTF-16 surrogate pair in JSON".into());
                            }
                            let low = self.parse_hex4()?;
                            if !(0xDC00..=0xDFFF).contains(&low) {
                                return Err("invalid UTF-16 low surrogate in JSON".into());
                            }
                            let cp = 0x10000
                                + (((u32::from(code) - 0xD800) << 10) | (u32::from(low) - 0xDC00));
                            let ch = char::from_u32(cp)
                                .ok_or_else(|| "invalid Unicode scalar in JSON".to_string())?;
                            out.push(ch);
                        } else {
                            let ch = char::from_u32(u32::from(code))
                                .ok_or_else(|| "invalid Unicode scalar in JSON".to_string())?;
                            out.push(ch);
                        }
                    }
                    other => return Err(format!("invalid JSON escape \\{}", other as char)),
                }
            } else if b < 0x80 {
                out.push(b as char);
            } else {
                let start = self.pos - 1;
                while self.pos < self.bytes.len() && (self.bytes[self.pos] & 0xC0) == 0x80 {
                    self.pos += 1;
                }
                let s = std::str::from_utf8(&self.bytes[start..self.pos])
                    .map_err(|e| format!("invalid UTF-8 in JSON string: {e}"))?;
                out.push_str(s);
            }
        }
    }

    fn parse_hex4(&mut self) -> Result<u16, String> {
        let mut val = 0u16;
        for _ in 0..4 {
            let b = self
                .next()
                .ok_or_else(|| "truncated \\u escape in JSON".to_string())?;
            let digit = match b {
                b'0'..=b'9' => b - b'0',
                b'a'..=b'f' => b - b'a' + 10,
                b'A'..=b'F' => b - b'A' + 10,
                _ => return Err("invalid hex digit in \\u escape".into()),
            };
            val = (val << 4) | u16::from(digit);
        }
        Ok(val)
    }

    fn parse_array(&mut self) -> Result<JsonValue, String> {
        if self.depth >= MAX_JSON_DEPTH {
            return Err("JSON nesting depth exceeded".into());
        }
        self.depth += 1;
        self.pos += 1; // '['
        let mut arr = Vec::new();
        self.skip_ws();
        if self.peek() == Some(b']') {
            self.pos += 1;
            self.depth -= 1;
            return Ok(JsonValue::Array(arr));
        }
        loop {
            arr.push(self.parse_value()?);
            self.skip_ws();
            match self.next() {
                Some(b',') => continue,
                Some(b']') => {
                    self.depth -= 1;
                    return Ok(JsonValue::Array(arr));
                }
                _ => return Err("expected ',' or ']' in JSON array".into()),
            }
        }
    }

    fn parse_object(&mut self) -> Result<JsonValue, String> {
        if self.depth >= MAX_JSON_DEPTH {
            return Err("JSON nesting depth exceeded".into());
        }
        self.depth += 1;
        self.pos += 1; // '{'
        let mut map = BTreeMap::new();
        self.skip_ws();
        if self.peek() == Some(b'}') {
            self.pos += 1;
            self.depth -= 1;
            return Ok(JsonValue::Object(map));
        }
        loop {
            self.skip_ws();
            let key = self.parse_string()?;
            self.skip_ws();
            if self.next() != Some(b':') {
                return Err("expected ':' in JSON object".into());
            }
            let val = self.parse_value()?;
            map.insert(key, val);
            self.skip_ws();
            match self.next() {
                Some(b',') => continue,
                Some(b'}') => {
                    self.depth -= 1;
                    return Ok(JsonValue::Object(map));
                }
                _ => return Err("expected ',' or '}' in JSON object".into()),
            }
        }
    }
}
