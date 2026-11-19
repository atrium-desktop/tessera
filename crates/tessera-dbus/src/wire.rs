//! D-Bus v1 little-endian wire protocol encoder and decoder.

use std::collections::HashMap;
use std::fmt;
use std::os::fd::OwnedFd;
use std::sync::Arc;

/// D-Bus error returned by wire decoding or remote method calls.
#[derive(Debug, Clone)]
pub enum WireError {
    InvalidFormat(String),
    InvalidSignature(String),
    TypeMismatch { expected: String, actual: String },
    MissingField(&'static str),
}

impl fmt::Display for WireError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidFormat(msg) => write!(f, "invalid D-Bus wire format: {msg}"),
            Self::InvalidSignature(sig) => write!(f, "invalid D-Bus signature: {sig:?}"),
            Self::TypeMismatch { expected, actual } => {
                write!(f, "D-Bus type mismatch: expected {expected}, got {actual}")
            }
            Self::MissingField(name) => write!(f, "missing D-Bus header field: {name}"),
        }
    }
}

impl std::error::Error for WireError {}

/// D-Bus message type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum MessageType {
    MethodCall = 1,
    MethodReturn = 2,
    Error = 3,
    Signal = 4,
}

impl MessageType {
    pub fn from_u8(v: u8) -> Result<Self, WireError> {
        match v {
            1 => Ok(Self::MethodCall),
            2 => Ok(Self::MethodReturn),
            3 => Ok(Self::Error),
            4 => Ok(Self::Signal),
            other => Err(WireError::InvalidFormat(format!(
                "unknown message type {other}"
            ))),
        }
    }
}

/// Validated D-Bus object path.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct OwnedObjectPath(String);

impl OwnedObjectPath {
    pub fn new(path: impl Into<String>) -> Result<Self, WireError> {
        let s = path.into();
        validate_object_path(&s)?;
        Ok(Self(s))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_inner(self) -> String {
        self.0
    }
}

impl fmt::Display for OwnedObjectPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for OwnedObjectPath {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl TryFrom<&str> for OwnedObjectPath {
    type Error = WireError;
    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl TryFrom<String> for OwnedObjectPath {
    type Error = WireError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

fn validate_object_path(path: &str) -> Result<(), WireError> {
    if !path.starts_with('/') {
        return Err(WireError::InvalidFormat(format!(
            "object path must start with '/': {path:?}"
        )));
    }
    if path.len() > 1 && path.ends_with('/') {
        return Err(WireError::InvalidFormat(format!(
            "object path must not end with '/': {path:?}"
        )));
    }
    if path.contains("//") {
        return Err(WireError::InvalidFormat(format!(
            "object path must not contain '//': {path:?}"
        )));
    }
    Ok(())
}

/// Dynamic D-Bus value.
#[derive(Debug, Clone)]
pub enum Value {
    Byte(u8),
    Bool(bool),
    Int16(i16),
    Uint16(u16),
    Int32(i32),
    Uint32(u32),
    Int64(i64),
    Uint64(u64),
    Double(f64),
    String(String),
    ObjectPath(OwnedObjectPath),
    Signature(String),
    Fd(Arc<OwnedFd>),
    Array(String, Vec<Value>),
    Struct(Vec<Value>),
    DictEntry(Box<Value>, Box<Value>),
    Variant(Box<Value>),
}

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Byte(a), Self::Byte(b)) => a == b,
            (Self::Bool(a), Self::Bool(b)) => a == b,
            (Self::Int16(a), Self::Int16(b)) => a == b,
            (Self::Uint16(a), Self::Uint16(b)) => a == b,
            (Self::Int32(a), Self::Int32(b)) => a == b,
            (Self::Uint32(a), Self::Uint32(b)) => a == b,
            (Self::Int64(a), Self::Int64(b)) => a == b,
            (Self::Uint64(a), Self::Uint64(b)) => a == b,
            (Self::Double(a), Self::Double(b)) => a == b,
            (Self::String(a), Self::String(b)) => a == b,
            (Self::ObjectPath(a), Self::ObjectPath(b)) => a == b,
            (Self::Signature(a), Self::Signature(b)) => a == b,
            (Self::Fd(a), Self::Fd(b)) => Arc::ptr_eq(a, b),
            (Self::Array(sa, va), Self::Array(sb, vb)) => sa == sb && va == vb,
            (Self::Struct(a), Self::Struct(b)) => a == b,
            (Self::DictEntry(ka, va), Self::DictEntry(kb, vb)) => ka == kb && va == vb,
            (Self::Variant(a), Self::Variant(b)) => a == b,
            _ => false,
        }
    }
}

impl Value {
    pub fn signature(&self) -> String {
        match self {
            Self::Byte(_) => "y".into(),
            Self::Bool(_) => "b".into(),
            Self::Int16(_) => "n".into(),
            Self::Uint16(_) => "q".into(),
            Self::Int32(_) => "i".into(),
            Self::Uint32(_) => "u".into(),
            Self::Int64(_) => "x".into(),
            Self::Uint64(_) => "t".into(),
            Self::Double(_) => "d".into(),
            Self::String(_) => "s".into(),
            Self::ObjectPath(_) => "o".into(),
            Self::Signature(_) => "g".into(),
            Self::Fd(_) => "h".into(),
            Self::Array(elem_sig, _) => format!("a{elem_sig}"),
            Self::Struct(fields) => {
                let mut s = String::from("(");
                for f in fields {
                    s.push_str(&f.signature());
                }
                s.push(')');
                s
            }
            Self::DictEntry(k, v) => format!("{{{}{}}}", k.signature(), v.signature()),
            Self::Variant(_) => "v".into(),
        }
    }

    pub fn type_name(&self) -> &'static str {
        match self {
            Self::Byte(_) => "byte",
            Self::Bool(_) => "bool",
            Self::Int16(_) => "i16",
            Self::Uint16(_) => "u16",
            Self::Int32(_) => "i32",
            Self::Uint32(_) => "u32",
            Self::Int64(_) => "i64",
            Self::Uint64(_) => "u64",
            Self::Double(_) => "f64",
            Self::String(_) => "string",
            Self::ObjectPath(_) => "object_path",
            Self::Signature(_) => "signature",
            Self::Fd(_) => "fd",
            Self::Array(_, _) => "array",
            Self::Struct(_) => "struct",
            Self::DictEntry(_, _) => "dict_entry",
            Self::Variant(_) => "variant",
        }
    }

    pub fn unwrap_variant(&self) -> &Self {
        let mut cur = self;
        while let Self::Variant(inner) = cur {
            cur = inner;
        }
        cur
    }

    pub fn as_str(&self) -> Option<&str> {
        match self.unwrap_variant() {
            Self::String(s) => Some(s.as_str()),
            Self::ObjectPath(p) => Some(p.as_str()),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self.unwrap_variant() {
            Self::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_u32(&self) -> Option<u32> {
        match self.unwrap_variant() {
            Self::Uint32(v) => Some(*v),
            _ => None,
        }
    }

    pub fn as_i32(&self) -> Option<i32> {
        match self.unwrap_variant() {
            Self::Int32(v) => Some(*v),
            _ => None,
        }
    }

    pub fn as_u8(&self) -> Option<u8> {
        match self.unwrap_variant() {
            Self::Byte(v) => Some(*v),
            _ => None,
        }
    }
}

/// Trait for types that can be encoded into D-Bus values.
pub trait ToValue {
    fn dbus_signature() -> String;
    fn to_value(&self) -> Value;
}

/// Trait for types that can be decoded from D-Bus values.
pub trait FromValue: Sized {
    fn from_value(value: Value) -> Result<Self, WireError>;
}

/// Trait for method argument tuples / structs.
pub trait ToArgs {
    fn to_args(&self) -> Vec<Value>;
}

/// Trait for method return tuples / structs.
pub trait FromArgs: Sized {
    fn from_args(args: Vec<Value>) -> Result<Self, WireError>;
}

impl ToArgs for () {
    fn to_args(&self) -> Vec<Value> {
        Vec::new()
    }
}

impl FromArgs for () {
    fn from_args(_args: Vec<Value>) -> Result<Self, WireError> {
        Ok(())
    }
}

impl<T: ToValue> ToArgs for (T,) {
    fn to_args(&self) -> Vec<Value> {
        vec![self.0.to_value()]
    }
}

impl<T: ToValue + ?Sized> ToArgs for &T {
    fn to_args(&self) -> Vec<Value> {
        vec![(*self).to_value()]
    }
}

impl<A: ToValue, B: ToValue> ToArgs for (A, B) {
    fn to_args(&self) -> Vec<Value> {
        vec![self.0.to_value(), self.1.to_value()]
    }
}

impl<A: ToValue, B: ToValue, C: ToValue> ToArgs for (A, B, C) {
    fn to_args(&self) -> Vec<Value> {
        vec![self.0.to_value(), self.1.to_value(), self.2.to_value()]
    }
}

impl<A: ToValue, B: ToValue, C: ToValue, D: ToValue> ToArgs for (A, B, C, D) {
    fn to_args(&self) -> Vec<Value> {
        vec![
            self.0.to_value(),
            self.1.to_value(),
            self.2.to_value(),
            self.3.to_value(),
        ]
    }
}

impl<A: ToValue, B: ToValue, C: ToValue, D: ToValue, E: ToValue> ToArgs for (A, B, C, D, E) {
    fn to_args(&self) -> Vec<Value> {
        vec![
            self.0.to_value(),
            self.1.to_value(),
            self.2.to_value(),
            self.3.to_value(),
            self.4.to_value(),
        ]
    }
}

impl<T: FromValue> FromArgs for T {
    fn from_args(mut args: Vec<Value>) -> Result<Self, WireError> {
        if args.is_empty() {
            return Err(WireError::InvalidFormat(
                "expected 1 return arg, got 0".into(),
            ));
        }
        T::from_value(args.remove(0))
    }
}

impl ToArgs for Vec<Value> {
    fn to_args(&self) -> Vec<Value> {
        self.clone()
    }
}

impl ToArgs for &[Value] {
    fn to_args(&self) -> Vec<Value> {
        self.to_vec()
    }
}

impl ToValue for Value {
    fn dbus_signature() -> String {
        "v".into()
    }
    fn to_value(&self) -> Value {
        self.clone()
    }
}

impl FromValue for Value {
    fn from_value(value: Value) -> Result<Self, WireError> {
        Ok(value)
    }
}

macro_rules! impl_scalar_value {
    ($ty:ty, $sig:literal, $var:ident, $name:literal) => {
        impl ToValue for $ty {
            fn dbus_signature() -> String {
                $sig.into()
            }
            fn to_value(&self) -> Value {
                Value::$var(*self)
            }
        }
        impl FromValue for $ty {
            fn from_value(value: Value) -> Result<Self, WireError> {
                match value {
                    Value::$var(v) => Ok(v),
                    Value::Variant(inner) => Self::from_value(*inner),
                    other => Err(WireError::TypeMismatch {
                        expected: $name.into(),
                        actual: other.type_name().into(),
                    }),
                }
            }
        }
    };
}

impl_scalar_value!(u8, "y", Byte, "byte");
impl_scalar_value!(bool, "b", Bool, "bool");
impl_scalar_value!(i16, "n", Int16, "i16");
impl_scalar_value!(u16, "q", Uint16, "u16");
impl_scalar_value!(i32, "i", Int32, "i32");
impl_scalar_value!(u32, "u", Uint32, "u32");
impl_scalar_value!(i64, "x", Int64, "i64");
impl_scalar_value!(u64, "t", Uint64, "u64");
impl_scalar_value!(f64, "d", Double, "f64");

impl ToValue for str {
    fn dbus_signature() -> String {
        "s".into()
    }
    fn to_value(&self) -> Value {
        Value::String(self.to_string())
    }
}

impl ToValue for &str {
    fn dbus_signature() -> String {
        "s".into()
    }
    fn to_value(&self) -> Value {
        Value::String((*self).to_string())
    }
}

impl ToValue for String {
    fn dbus_signature() -> String {
        "s".into()
    }
    fn to_value(&self) -> Value {
        Value::String(self.clone())
    }
}

impl FromValue for String {
    fn from_value(value: Value) -> Result<Self, WireError> {
        match value {
            Value::String(s) => Ok(s),
            Value::ObjectPath(p) => Ok(p.into_inner()),
            Value::Signature(s) => Ok(s),
            Value::Variant(inner) => Self::from_value(*inner),
            other => Err(WireError::TypeMismatch {
                expected: "string".into(),
                actual: other.type_name().into(),
            }),
        }
    }
}

impl ToValue for OwnedObjectPath {
    fn dbus_signature() -> String {
        "o".into()
    }
    fn to_value(&self) -> Value {
        Value::ObjectPath(self.clone())
    }
}

impl FromValue for OwnedObjectPath {
    fn from_value(value: Value) -> Result<Self, WireError> {
        match value {
            Value::ObjectPath(p) => Ok(p),
            Value::Variant(inner) => Self::from_value(*inner),
            other => Err(WireError::TypeMismatch {
                expected: "object_path".into(),
                actual: other.type_name().into(),
            }),
        }
    }
}

impl FromValue for OwnedFd {
    fn from_value(value: Value) -> Result<Self, WireError> {
        match value {
            Value::Fd(fd) => match Arc::try_unwrap(fd) {
                Ok(owned) => Ok(owned),
                Err(shared) => shared
                    .try_clone()
                    .map_err(|e| WireError::InvalidFormat(format!("dup fd: {e}"))),
            },
            Value::Variant(inner) => Self::from_value(*inner),
            other => Err(WireError::TypeMismatch {
                expected: "fd".into(),
                actual: other.type_name().into(),
            }),
        }
    }
}

impl<T: ToValue> ToValue for Vec<T> {
    fn dbus_signature() -> String {
        format!("a{}", T::dbus_signature())
    }
    fn to_value(&self) -> Value {
        Value::Array(
            T::dbus_signature(),
            self.iter().map(ToValue::to_value).collect(),
        )
    }
}

impl<T: ToValue> ToValue for &[T] {
    fn dbus_signature() -> String {
        format!("a{}", T::dbus_signature())
    }
    fn to_value(&self) -> Value {
        Value::Array(
            T::dbus_signature(),
            self.iter().map(ToValue::to_value).collect(),
        )
    }
}

impl<T: FromValue> FromValue for Vec<T> {
    fn from_value(value: Value) -> Result<Self, WireError> {
        match value {
            Value::Array(_, items) => items.into_iter().map(T::from_value).collect(),
            Value::Variant(inner) => Self::from_value(*inner),
            other => Err(WireError::TypeMismatch {
                expected: "array".into(),
                actual: other.type_name().into(),
            }),
        }
    }
}

impl<K: ToValue, V: ToValue, S: std::hash::BuildHasher> ToValue for HashMap<K, V, S> {
    fn dbus_signature() -> String {
        format!("a{{{}{}}}", K::dbus_signature(), V::dbus_signature())
    }
    fn to_value(&self) -> Value {
        let entry_sig = format!("{{{}{}}}", K::dbus_signature(), V::dbus_signature());
        let entries = self
            .iter()
            .map(|(k, v)| Value::DictEntry(Box::new(k.to_value()), Box::new(v.to_value())))
            .collect();
        Value::Array(entry_sig, entries)
    }
}

impl<K: FromValue + Eq + std::hash::Hash, V: FromValue> FromValue for HashMap<K, V> {
    fn from_value(value: Value) -> Result<Self, WireError> {
        match value {
            Value::Array(_, items) => {
                let mut map = HashMap::with_capacity(items.len());
                for item in items {
                    match item {
                        Value::DictEntry(k, v) => {
                            map.insert(K::from_value(*k)?, V::from_value(*v)?);
                        }
                        other => {
                            return Err(WireError::TypeMismatch {
                                expected: "dict_entry".into(),
                                actual: other.type_name().into(),
                            });
                        }
                    }
                }
                Ok(map)
            }
            Value::Variant(inner) => Self::from_value(*inner),
            other => Err(WireError::TypeMismatch {
                expected: "dict".into(),
                actual: other.type_name().into(),
            }),
        }
    }
}

impl<A: ToValue, B: ToValue> ToValue for (A, B) {
    fn dbus_signature() -> String {
        format!("({}{})", A::dbus_signature(), B::dbus_signature())
    }
    fn to_value(&self) -> Value {
        Value::Struct(vec![self.0.to_value(), self.1.to_value()])
    }
}

impl<A: FromValue, B: FromValue> FromValue for (A, B) {
    fn from_value(value: Value) -> Result<Self, WireError> {
        match value {
            Value::Struct(fields) if fields.len() == 2 => {
                let mut it = fields.into_iter();
                if let (Some(fa), Some(fb)) = (it.next(), it.next()) {
                    let a = A::from_value(fa)?;
                    let b = B::from_value(fb)?;
                    Ok((a, b))
                } else {
                    Err(WireError::TypeMismatch {
                        expected: "2-tuple struct".into(),
                        actual: "empty struct".into(),
                    })
                }
            }
            Value::Variant(inner) => Self::from_value(*inner),
            other => Err(WireError::TypeMismatch {
                expected: "2-tuple struct".into(),
                actual: other.type_name().into(),
            }),
        }
    }
}

impl<A: ToValue, B: ToValue, C: ToValue> ToValue for (A, B, C) {
    fn dbus_signature() -> String {
        format!(
            "({}{}{})",
            A::dbus_signature(),
            B::dbus_signature(),
            C::dbus_signature()
        )
    }
    fn to_value(&self) -> Value {
        Value::Struct(vec![
            self.0.to_value(),
            self.1.to_value(),
            self.2.to_value(),
        ])
    }
}

impl<A: FromValue, B: FromValue, C: FromValue> FromValue for (A, B, C) {
    fn from_value(value: Value) -> Result<Self, WireError> {
        match value {
            Value::Struct(fields) if fields.len() == 3 => {
                let mut it = fields.into_iter();
                if let (Some(fa), Some(fb), Some(fc)) = (it.next(), it.next(), it.next()) {
                    let a = A::from_value(fa)?;
                    let b = B::from_value(fb)?;
                    let c = C::from_value(fc)?;
                    Ok((a, b, c))
                } else {
                    Err(WireError::TypeMismatch {
                        expected: "3-tuple struct".into(),
                        actual: "empty struct".into(),
                    })
                }
            }
            Value::Variant(inner) => Self::from_value(*inner),
            other => Err(WireError::TypeMismatch {
                expected: "3-tuple struct".into(),
                actual: other.type_name().into(),
            }),
        }
    }
}

/// Decoded D-Bus message.
#[derive(Debug, Clone)]
pub struct Message {
    pub msg_type: MessageType,
    pub flags: u8,
    pub serial: u32,
    pub path: Option<OwnedObjectPath>,
    pub interface: Option<String>,
    pub member: Option<String>,
    pub error_name: Option<String>,
    pub reply_serial: Option<u32>,
    pub destination: Option<String>,
    pub sender: Option<String>,
    pub signature: Option<String>,
    pub body: Vec<Value>,
}

impl Message {
    pub fn method_call(
        serial: u32,
        destination: Option<&str>,
        path: &str,
        interface: Option<&str>,
        member: &str,
        body: Vec<Value>,
    ) -> Result<Self, WireError> {
        Ok(Self {
            msg_type: MessageType::MethodCall,
            flags: 0,
            serial,
            path: Some(OwnedObjectPath::new(path)?),
            interface: interface.map(str::to_string),
            member: Some(member.to_string()),
            error_name: None,
            reply_serial: None,
            destination: destination.map(str::to_string),
            sender: None,
            signature: None,
            body,
        })
    }

    pub fn method_return(
        serial: u32,
        reply_serial: u32,
        destination: Option<&str>,
        body: Vec<Value>,
    ) -> Self {
        Self {
            msg_type: MessageType::MethodReturn,
            flags: 0,
            serial,
            path: None,
            interface: None,
            member: None,
            error_name: None,
            reply_serial: Some(reply_serial),
            destination: destination.map(str::to_string),
            sender: None,
            signature: None,
            body,
        }
    }

    pub fn error_reply(
        serial: u32,
        reply_serial: u32,
        destination: Option<&str>,
        error_name: &str,
        message: &str,
    ) -> Self {
        Self {
            msg_type: MessageType::Error,
            flags: 0,
            serial,
            path: None,
            interface: None,
            member: None,
            error_name: Some(error_name.to_string()),
            reply_serial: Some(reply_serial),
            destination: destination.map(str::to_string),
            sender: None,
            signature: None,
            body: vec![Value::String(message.to_string())],
        }
    }

    pub fn signal(
        serial: u32,
        destination: Option<&str>,
        path: &str,
        interface: &str,
        member: &str,
        body: Vec<Value>,
    ) -> Result<Self, WireError> {
        Ok(Self {
            msg_type: MessageType::Signal,
            flags: 0,
            serial,
            path: Some(OwnedObjectPath::new(path)?),
            interface: Some(interface.to_string()),
            member: Some(member.to_string()),
            error_name: None,
            reply_serial: None,
            destination: destination.map(str::to_string),
            sender: None,
            signature: None,
            body,
        })
    }

    /// Encode this message into wire bytes + outgoing file descriptors.
    pub fn encode(&self) -> Result<(Vec<u8>, Vec<Arc<OwnedFd>>), WireError> {
        let mut body_enc = Encoder::new();
        let mut body_sig = String::new();
        for val in &self.body {
            body_sig.push_str(&val.signature());
            body_enc.write_value(val)?;
        }

        let mut fields: Vec<Value> = Vec::new();
        if let Some(ref p) = self.path {
            fields.push(header_field(1, Value::ObjectPath(p.clone())));
        }
        if let Some(ref i) = self.interface {
            fields.push(header_field(2, Value::String(i.clone())));
        }
        if let Some(ref m) = self.member {
            fields.push(header_field(3, Value::String(m.clone())));
        }
        if let Some(ref e) = self.error_name {
            fields.push(header_field(4, Value::String(e.clone())));
        }
        if let Some(rs) = self.reply_serial {
            fields.push(header_field(5, Value::Uint32(rs)));
        }
        if let Some(ref d) = self.destination {
            fields.push(header_field(6, Value::String(d.clone())));
        }
        if let Some(ref s) = self.sender {
            fields.push(header_field(7, Value::String(s.clone())));
        }
        if !body_sig.is_empty() {
            fields.push(header_field(8, Value::Signature(body_sig)));
        }
        if !body_enc.fds.is_empty() {
            fields.push(header_field(9, Value::Uint32(body_enc.fds.len() as u32)));
        }

        let mut hdr = Encoder::new();
        hdr.buf.push(b'l'); // little-endian
        hdr.buf.push(self.msg_type as u8);
        hdr.buf.push(self.flags);
        hdr.buf.push(1); // D-Bus protocol version 1
        hdr.write_u32(body_enc.buf.len() as u32);
        hdr.write_u32(self.serial);
        hdr.write_value(&Value::Array("(yv)".into(), fields))?;
        hdr.align(8);
        hdr.buf.extend_from_slice(&body_enc.buf);

        Ok((hdr.buf, body_enc.fds))
    }

    /// Decode a complete message (header + body) from wire bytes and received FDs.
    pub fn decode(bytes: &[u8], fds: Vec<OwnedFd>) -> Result<Self, WireError> {
        if bytes.len() < 16 {
            return Err(WireError::InvalidFormat(
                "message shorter than 16 bytes".into(),
            ));
        }
        if bytes[0] != b'l' {
            return Err(WireError::InvalidFormat(format!(
                "unsupported endianness: {:?}",
                bytes[0] as char
            )));
        }
        let msg_type = MessageType::from_u8(bytes[1])?;
        let flags = bytes[2];
        let version = bytes[3];
        if version != 1 {
            return Err(WireError::InvalidFormat(format!(
                "unsupported D-Bus version {version}"
            )));
        }
        let body_len = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]) as usize;
        let serial = u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]);
        let shared_fds: Vec<Arc<OwnedFd>> = fds.into_iter().map(Arc::new).collect();

        let mut dec = Decoder::new(bytes, 12, &shared_fds);
        let fields_val = dec.read_by_sig("a(yv)")?;
        dec.align(8)?;

        if dec.pos + body_len != bytes.len() {
            return Err(WireError::InvalidFormat(format!(
                "body length mismatch: header says {body_len}, remaining {}",
                bytes.len().saturating_sub(dec.pos)
            )));
        }

        let mut path = None;
        let mut interface = None;
        let mut member = None;
        let mut error_name = None;
        let mut reply_serial = None;
        let mut destination = None;
        let mut sender = None;
        let mut signature = None;

        if let Value::Array(_, entries) = fields_val {
            for entry in entries {
                if let Value::Struct(mut kv) = entry
                    && let (Some(val), Some(code_val)) = (kv.pop(), kv.pop())
                    && kv.is_empty()
                {
                    let code = match code_val {
                        Value::Byte(c) => c,
                        _ => continue,
                    };
                    match code {
                        1 => {
                            if let Value::ObjectPath(p) = val.unwrap_variant() {
                                path = Some(p.clone());
                            }
                        }
                        2 => {
                            if let Value::String(s) = val.unwrap_variant() {
                                interface = Some(s.clone());
                            }
                        }
                        3 => {
                            if let Value::String(s) = val.unwrap_variant() {
                                member = Some(s.clone());
                            }
                        }
                        4 => {
                            if let Value::String(s) = val.unwrap_variant() {
                                error_name = Some(s.clone());
                            }
                        }
                        5 => {
                            if let Value::Uint32(n) = val.unwrap_variant() {
                                reply_serial = Some(*n);
                            }
                        }
                        6 => {
                            if let Value::String(s) = val.unwrap_variant() {
                                destination = Some(s.clone());
                            }
                        }
                        7 => {
                            if let Value::String(s) = val.unwrap_variant() {
                                sender = Some(s.clone());
                            }
                        }
                        8 => {
                            if let Value::Signature(s) = val.unwrap_variant() {
                                signature = Some(s.clone());
                            }
                        }
                        _ => {}
                    }
                }
            }
        }

        let mut body = Vec::new();
        if let Some(ref sig) = signature {
            let mut rest = sig.as_str();
            while !rest.is_empty() {
                let (one, next) = split_first_signature(rest)?;
                body.push(dec.read_by_sig(one)?);
                rest = next;
            }
        }

        Ok(Self {
            msg_type,
            flags,
            serial,
            path,
            interface,
            member,
            error_name,
            reply_serial,
            destination,
            sender,
            signature,
            body,
        })
    }
}

fn header_field(code: u8, val: Value) -> Value {
    Value::Struct(vec![Value::Byte(code), Value::Variant(Box::new(val))])
}

/// Compute total frame length (header + padded fields + body) from the first 16 bytes.
pub fn frame_total_len(prefix16: &[u8; 16]) -> Result<usize, WireError> {
    if prefix16[0] != b'l' {
        return Err(WireError::InvalidFormat(format!(
            "unsupported endianness: {:?}",
            prefix16[0] as char
        )));
    }
    let body_len =
        u32::from_le_bytes([prefix16[4], prefix16[5], prefix16[6], prefix16[7]]) as usize;
    let fields_len =
        u32::from_le_bytes([prefix16[12], prefix16[13], prefix16[14], prefix16[15]]) as usize;
    let header_unpadded = 16usize
        .checked_add(fields_len)
        .ok_or_else(|| WireError::InvalidFormat("header overflow".into()))?;
    let header_padded = (header_unpadded + 7) & !7;
    header_padded
        .checked_add(body_len)
        .ok_or_else(|| WireError::InvalidFormat("frame overflow".into()))
}

fn sig_alignment(sig: &str) -> Result<usize, WireError> {
    let first = sig
        .as_bytes()
        .first()
        .copied()
        .ok_or_else(|| WireError::InvalidSignature(sig.into()))?;
    match first {
        b'y' | b'g' | b'v' => Ok(1),
        b'n' | b'q' => Ok(2),
        b'b' | b'i' | b'u' | b'h' | b's' | b'o' | b'a' => Ok(4),
        b'x' | b't' | b'd' | b'(' | b'{' => Ok(8),
        other => Err(WireError::InvalidSignature(format!(
            "unexpected type byte {:?}",
            other as char
        ))),
    }
}

fn split_first_signature(sig: &str) -> Result<(&str, &str), WireError> {
    let bytes = sig.as_bytes();
    if bytes.is_empty() {
        return Err(WireError::InvalidSignature(sig.into()));
    }
    match bytes[0] {
        b'y' | b'b' | b'n' | b'q' | b'i' | b'u' | b'x' | b't' | b'd' | b's' | b'o' | b'g'
        | b'h' | b'v' => Ok((&sig[..1], &sig[1..])),
        b'a' => {
            let (elem, rest) = split_first_signature(&sig[1..])?;
            let total = 1 + elem.len();
            Ok((&sig[..total], rest))
        }
        b'(' | b'{' => {
            let open = bytes[0];
            let close = if open == b'(' { b')' } else { b'}' };
            let mut depth = 0usize;
            for (idx, &b) in bytes.iter().enumerate() {
                if b == open {
                    depth += 1;
                } else if b == close {
                    depth -= 1;
                    if depth == 0 {
                        return Ok((&sig[..=idx], &sig[idx + 1..]));
                    }
                }
            }
            Err(WireError::InvalidSignature(sig.into()))
        }
        _ => Err(WireError::InvalidSignature(sig.into())),
    }
}

struct Encoder {
    buf: Vec<u8>,
    fds: Vec<Arc<OwnedFd>>,
}

impl Encoder {
    fn new() -> Self {
        Self {
            buf: Vec::new(),
            fds: Vec::new(),
        }
    }

    fn align(&mut self, alignment: usize) {
        let rem = self.buf.len() % alignment;
        if rem != 0 {
            let pad = alignment - rem;
            self.buf.resize(self.buf.len() + pad, 0);
        }
    }

    fn write_u16(&mut self, v: u16) {
        self.align(2);
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    fn write_u32(&mut self, v: u32) {
        self.align(4);
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    fn write_u64(&mut self, v: u64) {
        self.align(8);
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    fn write_value(&mut self, val: &Value) -> Result<(), WireError> {
        match val {
            Value::Byte(v) => self.buf.push(*v),
            Value::Bool(v) => self.write_u32(u32::from(*v)),
            Value::Int16(v) => self.write_u16(*v as u16),
            Value::Uint16(v) => self.write_u16(*v),
            Value::Int32(v) => self.write_u32(*v as u32),
            Value::Uint32(v) => self.write_u32(*v),
            Value::Int64(v) => self.write_u64(*v as u64),
            Value::Uint64(v) => self.write_u64(*v),
            Value::Double(v) => self.write_u64(v.to_bits()),
            Value::String(s) => {
                self.write_u32(s.len() as u32);
                self.buf.extend_from_slice(s.as_bytes());
                self.buf.push(0);
            }
            Value::ObjectPath(p) => {
                let s = p.as_str();
                self.write_u32(s.len() as u32);
                self.buf.extend_from_slice(s.as_bytes());
                self.buf.push(0);
            }
            Value::Signature(s) => {
                if s.len() > 255 {
                    return Err(WireError::InvalidSignature(s.clone()));
                }
                self.buf.push(s.len() as u8);
                self.buf.extend_from_slice(s.as_bytes());
                self.buf.push(0);
            }
            Value::Fd(fd) => {
                let idx = self.fds.len() as u32;
                self.fds.push(Arc::clone(fd));
                self.write_u32(idx);
            }
            Value::Array(elem_sig, items) => {
                self.align(4);
                let len_pos = self.buf.len();
                self.buf.extend_from_slice(&[0u8; 4]);
                let elem_align = sig_alignment(elem_sig)?;
                self.align(elem_align);
                let start = self.buf.len();
                for item in items {
                    self.write_value(item)?;
                }
                let byte_len = (self.buf.len() - start) as u32;
                self.buf[len_pos..len_pos + 4].copy_from_slice(&byte_len.to_le_bytes());
            }
            Value::Struct(fields) => {
                self.align(8);
                for field in fields {
                    self.write_value(field)?;
                }
            }
            Value::DictEntry(k, v) => {
                self.align(8);
                self.write_value(k)?;
                self.write_value(v)?;
            }
            Value::Variant(inner) => {
                let sig = inner.signature();
                self.write_value(&Value::Signature(sig))?;
                self.write_value(inner)?;
            }
        }
        Ok(())
    }
}

struct Decoder<'a> {
    buf: &'a [u8],
    pos: usize,
    fds: &'a [Arc<OwnedFd>],
}

impl<'a> Decoder<'a> {
    fn new(buf: &'a [u8], pos: usize, fds: &'a [Arc<OwnedFd>]) -> Self {
        Self { buf, pos, fds }
    }

    fn align(&mut self, alignment: usize) -> Result<(), WireError> {
        let rem = self.pos % alignment;
        if rem != 0 {
            let next = self.pos + (alignment - rem);
            if next > self.buf.len() {
                return Err(WireError::InvalidFormat(
                    "unexpected EOF while aligning".into(),
                ));
            }
            self.pos = next;
        }
        Ok(())
    }

    fn read_bytes(&mut self, n: usize) -> Result<&'a [u8], WireError> {
        let end = self
            .pos
            .checked_add(n)
            .ok_or_else(|| WireError::InvalidFormat("offset overflow".into()))?;
        if end > self.buf.len() {
            return Err(WireError::InvalidFormat("unexpected EOF".into()));
        }
        let slice = &self.buf[self.pos..end];
        self.pos = end;
        Ok(slice)
    }

    fn read_u8(&mut self) -> Result<u8, WireError> {
        Ok(self.read_bytes(1)?[0])
    }

    fn read_u16(&mut self) -> Result<u16, WireError> {
        self.align(2)?;
        let b = self.read_bytes(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    fn read_u32(&mut self) -> Result<u32, WireError> {
        self.align(4)?;
        let b = self.read_bytes(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn read_u64(&mut self) -> Result<u64, WireError> {
        self.align(8)?;
        let b = self.read_bytes(8)?;
        Ok(u64::from_le_bytes([
            b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
        ]))
    }

    fn read_string(&mut self) -> Result<String, WireError> {
        let len = self.read_u32()? as usize;
        let bytes = self.read_bytes(len)?;
        let nul = self.read_u8()?;
        if nul != 0 {
            return Err(WireError::InvalidFormat(
                "string missing NUL terminator".into(),
            ));
        }
        String::from_utf8(bytes.to_vec())
            .map_err(|e| WireError::InvalidFormat(format!("invalid UTF-8 string: {e}")))
    }

    fn read_signature(&mut self) -> Result<String, WireError> {
        let len = self.read_u8()? as usize;
        let bytes = self.read_bytes(len)?;
        let nul = self.read_u8()?;
        if nul != 0 {
            return Err(WireError::InvalidFormat("signature missing NUL".into()));
        }
        String::from_utf8(bytes.to_vec())
            .map_err(|e| WireError::InvalidFormat(format!("invalid UTF-8 signature: {e}")))
    }

    fn read_by_sig(&mut self, sig: &str) -> Result<Value, WireError> {
        let first = sig
            .as_bytes()
            .first()
            .copied()
            .ok_or_else(|| WireError::InvalidSignature(sig.into()))?;
        match first {
            b'y' => Ok(Value::Byte(self.read_u8()?)),
            b'b' => Ok(Value::Bool(self.read_u32()? != 0)),
            b'n' => Ok(Value::Int16(self.read_u16()? as i16)),
            b'q' => Ok(Value::Uint16(self.read_u16()?)),
            b'i' => Ok(Value::Int32(self.read_u32()? as i32)),
            b'u' => Ok(Value::Uint32(self.read_u32()?)),
            b'x' => Ok(Value::Int64(self.read_u64()? as i64)),
            b't' => Ok(Value::Uint64(self.read_u64()?)),
            b'd' => Ok(Value::Double(f64::from_bits(self.read_u64()?))),
            b's' => Ok(Value::String(self.read_string()?)),
            b'o' => Ok(Value::ObjectPath(OwnedObjectPath::new(
                self.read_string()?,
            )?)),
            b'g' => Ok(Value::Signature(self.read_signature()?)),
            b'h' => {
                let idx = self.read_u32()? as usize;
                let fd = self
                    .fds
                    .get(idx)
                    .ok_or_else(|| WireError::InvalidFormat(format!("bad fd index {idx}")))?;
                Ok(Value::Fd(Arc::clone(fd)))
            }
            b'v' => {
                let inner_sig = self.read_signature()?;
                let inner = self.read_by_sig(&inner_sig)?;
                Ok(Value::Variant(Box::new(inner)))
            }
            b'a' => {
                let elem_sig = &sig[1..];
                let byte_len = self.read_u32()? as usize;
                let elem_align = sig_alignment(elem_sig)?;
                self.align(elem_align)?;
                let end = self.pos + byte_len;
                if end > self.buf.len() {
                    return Err(WireError::InvalidFormat("array exceeds buffer".into()));
                }
                let mut items = Vec::new();
                while self.pos < end {
                    items.push(self.read_by_sig(elem_sig)?);
                }
                if self.pos != end {
                    return Err(WireError::InvalidFormat("array overread".into()));
                }
                Ok(Value::Array(elem_sig.to_string(), items))
            }
            b'(' => {
                self.align(8)?;
                let inner = &sig[1..sig.len() - 1];
                let mut fields = Vec::new();
                let mut rest = inner;
                while !rest.is_empty() {
                    let (one, next) = split_first_signature(rest)?;
                    fields.push(self.read_by_sig(one)?);
                    rest = next;
                }
                Ok(Value::Struct(fields))
            }
            b'{' => {
                self.align(8)?;
                let inner = &sig[1..sig.len() - 1];
                let (k_sig, v_sig) = split_first_signature(inner)?;
                let k = self.read_by_sig(k_sig)?;
                let v = self.read_by_sig(v_sig)?;
                Ok(Value::DictEntry(Box::new(k), Box::new(v)))
            }
            _ => Err(WireError::InvalidSignature(sig.into())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_complex_message() {
        let mut dict = HashMap::new();
        dict.insert(
            "Title".to_string(),
            Value::Variant(Box::new(Value::String("Track".into()))),
        );
        dict.insert(
            "Count".to_string(),
            Value::Variant(Box::new(Value::Uint32(42))),
        );

        let msg = Message::method_call(
            7,
            Some("org.freedesktop.DBus"),
            "/org/freedesktop/DBus",
            Some("org.freedesktop.DBus"),
            "TestMethod",
            vec![Value::String("hello".into()), dict.to_value()],
        )
        .unwrap();

        let (bytes, fds) = msg.encode().unwrap();
        assert!(fds.is_empty());
        let decoded = Message::decode(&bytes, Vec::new()).unwrap();
        assert_eq!(decoded.msg_type, MessageType::MethodCall);
        assert_eq!(decoded.serial, 7);
        assert_eq!(decoded.member.as_deref(), Some("TestMethod"));
        assert_eq!(decoded.body.len(), 2);
        assert_eq!(decoded.body[0].as_str(), Some("hello"));
    }
}
