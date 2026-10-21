//! The part of Avro that Iceberg's manifests need: records of null,
//! boolean, int, long, bytes and string, arrays and unions, in an object
//! container file with no compression.
//!
//! Encoding is driven by the schema, so a value that does not fit is
//! refused instead of written. The container's sync marker is the start of
//! a SHA-256 of the file's header and data, not a random number, so the
//! same records give the same bytes.

use std::collections::BTreeMap;

use serde_json::{Value as Json, json};
use sha2::{Digest, Sha256};

/// The first four bytes of an object container file.
pub const MAGIC: [u8; 4] = *b"Obj\x01";

/// Why bytes could not be written or read as Avro.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AvroError {
    /// A value of another shape than the schema's.
    #[error("a value does not fit the Avro schema: {expected} expected")]
    Mismatch {
        /// What the schema wanted there.
        expected: &'static str,
    },
    /// The bytes end early.
    #[error("the Avro bytes end early")]
    Truncated,
    /// A variable-length number longer than ten bytes.
    #[error("an Avro number runs past ten bytes")]
    Varint,
    /// A string that is not UTF-8.
    #[error("an Avro string is not UTF-8")]
    Utf8,
    /// A union branch past the union's types.
    #[error("Avro union branch {0} does not exist")]
    Branch(i64),
    /// Not an object container file.
    #[error("the bytes are not an Avro object container file")]
    Magic,
    /// The file was written with another schema or codec than the reader's.
    #[error("the Avro file was written with another {0} than the reader's")]
    Header(&'static str),
    /// A block does not end with the file's sync marker.
    #[error("an Avro block does not end with the file's sync marker")]
    Sync,
}

/// An Avro type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ty {
    /// `null`.
    Null,
    /// `boolean`.
    Boolean,
    /// `int`.
    Int,
    /// `long`.
    Long,
    /// `bytes`.
    Bytes,
    /// `string`.
    String,
    /// A record; Iceberg gives each field an id.
    Record {
        /// The record's name.
        name: &'static str,
        /// Its fields, in order.
        fields: Vec<FieldTy>,
    },
    /// An array; Iceberg gives its element an id.
    Array {
        /// The element type.
        items: Box<Ty>,
        /// The element's field id.
        element_id: i32,
    },
    /// A union of the types, by position.
    Union(Vec<Ty>),
}

/// A field of a record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldTy {
    /// Its name.
    pub name: &'static str,
    /// Its Iceberg field id.
    pub id: i32,
    /// Its type.
    pub ty: Ty,
}

impl FieldTy {
    /// A field that must have a value.
    pub fn required(name: &'static str, id: i32, ty: Ty) -> Self {
        Self { name, id, ty }
    }

    /// A field that may be null: a union of null and `ty`, null by default.
    pub fn optional(name: &'static str, id: i32, ty: Ty) -> Self {
        Self {
            name,
            id,
            ty: Ty::Union(vec![Ty::Null, ty]),
        }
    }
}

/// An Avro value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    /// `null`.
    Null,
    /// `boolean`.
    Boolean(bool),
    /// `int`.
    Int(i32),
    /// `long`.
    Long(i64),
    /// `bytes`.
    Bytes(Vec<u8>),
    /// `string`.
    String(String),
    /// A record's field values, in order.
    Record(Vec<Value>),
    /// An array's items.
    Array(Vec<Value>),
    /// A union's branch and its value.
    Union(usize, Box<Value>),
}

impl Value {
    /// The value of an optional field: null, or branch 1 holding `v`.
    pub fn opt(v: Option<Self>) -> Self {
        match v {
            None => Self::Union(0, Box::new(Self::Null)),
            Some(v) => Self::Union(1, Box::new(v)),
        }
    }
}

impl Ty {
    /// The schema as the container's header writes it.
    pub fn to_json(&self) -> Json {
        match self {
            Self::Null => json!("null"),
            Self::Boolean => json!("boolean"),
            Self::Int => json!("int"),
            Self::Long => json!("long"),
            Self::Bytes => json!("bytes"),
            Self::String => json!("string"),
            Self::Record { name, fields } => json!({
                "type": "record",
                "name": name,
                "fields": fields.iter().map(|f| {
                    let mut v = json!({"name": f.name, "type": f.ty.to_json(), "field-id": f.id});
                    if matches!(&f.ty, Self::Union(b) if b.first() == Some(&Self::Null)) {
                        v["default"] = Json::Null;
                    }
                    v
                }).collect::<Vec<_>>(),
            }),
            Self::Array { items, element_id } => json!({
                "type": "array",
                "items": items.to_json(),
                "element-id": element_id,
            }),
            Self::Union(branches) => Json::Array(branches.iter().map(Self::to_json).collect()),
        }
    }
}

/// Appends `v` as a zigzag variable-length number.
pub fn put_long(out: &mut Vec<u8>, v: i64) {
    let mut z = ((v << 1) ^ (v >> 63)) as u64;
    loop {
        let byte = (z & 0x7F) as u8;
        z >>= 7;
        if z == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

fn put_bytes(out: &mut Vec<u8>, b: &[u8]) {
    put_long(out, b.len() as i64);
    out.extend_from_slice(b);
}

/// Appends `v` encoded as `ty`.
pub fn encode(ty: &Ty, v: &Value, out: &mut Vec<u8>) -> Result<(), AvroError> {
    match (ty, v) {
        (Ty::Null, Value::Null) => {}
        (Ty::Boolean, Value::Boolean(b)) => out.push(u8::from(*b)),
        (Ty::Int, Value::Int(i)) => put_long(out, i64::from(*i)),
        (Ty::Long, Value::Long(l)) => put_long(out, *l),
        (Ty::Bytes, Value::Bytes(b)) => put_bytes(out, b),
        (Ty::String, Value::String(s)) => put_bytes(out, s.as_bytes()),
        (Ty::Record { fields, .. }, Value::Record(values)) if fields.len() == values.len() => {
            for (f, v) in fields.iter().zip(values) {
                encode(&f.ty, v, out)?;
            }
        }
        (Ty::Array { items, .. }, Value::Array(values)) => {
            if !values.is_empty() {
                put_long(out, values.len() as i64);
                for v in values {
                    encode(items, v, out)?;
                }
            }
            put_long(out, 0);
        }
        (Ty::Union(branches), Value::Union(i, v)) if *i < branches.len() => {
            put_long(out, *i as i64);
            encode(&branches[*i], v, out)?;
        }
        _ => {
            return Err(AvroError::Mismatch {
                expected: ty.kind(),
            });
        }
    }
    Ok(())
}

impl Ty {
    const fn kind(&self) -> &'static str {
        match self {
            Self::Null => "null",
            Self::Boolean => "boolean",
            Self::Int => "int",
            Self::Long => "long",
            Self::Bytes => "bytes",
            Self::String => "string",
            Self::Record { .. } => "a record of its fields",
            Self::Array { .. } => "array",
            Self::Union(_) => "a union branch",
        }
    }
}

/// Reads a zigzag variable-length number.
pub fn get_long(input: &mut &[u8]) -> Result<i64, AvroError> {
    let mut z: u64 = 0;
    for shift in (0..70).step_by(7) {
        let (&byte, rest) = input.split_first().ok_or(AvroError::Truncated)?;
        *input = rest;
        z |= u64::from(byte & 0x7F) << shift;
        if byte & 0x80 == 0 {
            return Ok((z >> 1) as i64 ^ -((z & 1) as i64));
        }
    }
    Err(AvroError::Varint)
}

fn get_bytes<'a>(input: &mut &'a [u8]) -> Result<&'a [u8], AvroError> {
    let n = usize::try_from(get_long(input)?).map_err(|_| AvroError::Truncated)?;
    if n > input.len() {
        return Err(AvroError::Truncated);
    }
    let (b, rest) = input.split_at(n);
    *input = rest;
    Ok(b)
}

/// The count of the next block of an array or map, or `None` at its end.
/// A negative count is followed by the block's size in bytes.
fn block_count(input: &mut &[u8]) -> Result<Option<i64>, AvroError> {
    let n = get_long(input)?;
    if n < 0 {
        get_long(input)?;
    }
    Ok((n != 0).then_some(n.saturating_abs()))
}

fn get_str(input: &mut &[u8]) -> Result<String, AvroError> {
    std::str::from_utf8(get_bytes(input)?)
        .map(str::to_owned)
        .map_err(|_| AvroError::Utf8)
}

/// Reads one value of `ty`.
pub fn decode(ty: &Ty, input: &mut &[u8]) -> Result<Value, AvroError> {
    Ok(match ty {
        Ty::Null => Value::Null,
        Ty::Boolean => {
            let (&b, rest) = input.split_first().ok_or(AvroError::Truncated)?;
            *input = rest;
            Value::Boolean(b != 0)
        }
        Ty::Int => Value::Int(i32::try_from(get_long(input)?).map_err(|_| AvroError::Varint)?),
        Ty::Long => Value::Long(get_long(input)?),
        Ty::Bytes => Value::Bytes(get_bytes(input)?.to_vec()),
        Ty::String => Value::String(get_str(input)?),
        Ty::Record { fields, .. } => Value::Record(
            fields
                .iter()
                .map(|f| decode(&f.ty, input))
                .collect::<Result<_, _>>()?,
        ),
        Ty::Array { items, .. } => {
            let mut values = Vec::new();
            while let Some(n) = block_count(input)? {
                for _ in 0..n {
                    values.push(decode(items, input)?);
                }
            }
            Value::Array(values)
        }
        Ty::Union(branches) => {
            let i = get_long(input)?;
            let branch = usize::try_from(i)
                .ok()
                .and_then(|b| branches.get(b).map(|t| (b, t)))
                .ok_or(AvroError::Branch(i))?;
            Value::Union(branch.0, Box::new(decode(branch.1, input)?))
        }
    })
}

/// An object container file of `records` of `ty`, with `metadata` (the
/// schema and codec are added). One data block, or none when empty.
pub fn write_container(
    ty: &Ty,
    metadata: &BTreeMap<String, String>,
    records: &[Value],
) -> Result<Vec<u8>, AvroError> {
    let mut meta: BTreeMap<&str, &[u8]> = metadata
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_bytes()))
        .collect();
    let schema = ty.to_json().to_string();
    meta.insert("avro.schema", schema.as_bytes());
    meta.insert("avro.codec", b"null");
    let mut header = MAGIC.to_vec();
    put_long(&mut header, meta.len() as i64);
    for (k, v) in &meta {
        put_bytes(&mut header, k.as_bytes());
        put_bytes(&mut header, v);
    }
    put_long(&mut header, 0);
    let mut data = Vec::new();
    for r in records {
        encode(ty, r, &mut data)?;
    }
    let digest = Sha256::new()
        .chain_update(&header)
        .chain_update(&data)
        .finalize();
    let sync = &digest[..16];
    let mut out = header;
    out.extend_from_slice(sync);
    if !records.is_empty() {
        put_long(&mut out, records.len() as i64);
        put_bytes(&mut out, &data);
        out.extend_from_slice(sync);
    }
    Ok(out)
}

/// The metadata and records of an object container file written with
/// schema `ty` and no compression.
pub fn read_container(
    ty: &Ty,
    bytes: &[u8],
) -> Result<(BTreeMap<String, String>, Vec<Value>), AvroError> {
    let mut input = bytes.strip_prefix(&MAGIC).ok_or(AvroError::Magic)?;
    let mut metadata = BTreeMap::new();
    while let Some(n) = block_count(&mut input)? {
        for _ in 0..n {
            let k = get_str(&mut input)?;
            metadata.insert(k, get_str(&mut input)?);
        }
    }
    if metadata.remove("avro.schema") != Some(ty.to_json().to_string()) {
        return Err(AvroError::Header("schema"));
    }
    if metadata.remove("avro.codec").is_some_and(|c| c != "null") {
        return Err(AvroError::Header("codec"));
    }
    let sync: [u8; 16] = input
        .get(..16)
        .ok_or(AvroError::Truncated)?
        .try_into()
        .unwrap_or_default();
    input = &input[16..];
    let mut records = Vec::new();
    while !input.is_empty() {
        let n = get_long(&mut input)?;
        let mut block = get_bytes(&mut input)?;
        for _ in 0..n {
            records.push(decode(ty, &mut block)?);
        }
        if input.get(..16) != Some(&sync[..]) {
            return Err(AvroError::Sync);
        }
        input = &input[16..];
    }
    Ok((metadata, records))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ty() -> Ty {
        Ty::Record {
            name: "r",
            fields: vec![
                FieldTy::required("n", 1, Ty::Null),
                FieldTy::required("b", 2, Ty::Boolean),
                FieldTy::required("i", 3, Ty::Int),
                FieldTy::required("l", 4, Ty::Long),
                FieldTy::required("y", 5, Ty::Bytes),
                FieldTy::required("s", 6, Ty::String),
                FieldTy::required(
                    "a",
                    7,
                    Ty::Array {
                        items: Box::new(Ty::Long),
                        element_id: 8,
                    },
                ),
                FieldTy::optional("o", 9, Ty::String),
            ],
        }
    }

    fn value(i: i64) -> Value {
        Value::Record(vec![
            Value::Null,
            Value::Boolean(i % 2 == 0),
            Value::Int(i32::try_from(i).unwrap()),
            Value::Long(-i * 1_000_000_007),
            Value::Bytes(vec![0, 1, 255]),
            Value::String(format!("é{i}")),
            Value::Array((0..i).map(Value::Long).collect()),
            Value::opt((i > 1).then(|| Value::String("x".into()))),
        ])
    }

    #[test]
    fn numbers_round_trip_through_zigzag() {
        for v in [
            0,
            -1,
            1,
            63,
            -64,
            64,
            i64::from(i32::MAX),
            i64::MIN,
            i64::MAX,
        ] {
            let mut out = Vec::new();
            put_long(&mut out, v);
            let mut input = out.as_slice();
            assert_eq!(get_long(&mut input), Ok(v), "{v}");
            assert!(input.is_empty());
        }
        let mut out = Vec::new();
        put_long(&mut out, -3);
        assert_eq!(out, [5], "zigzag puts the sign in the low bit");
        assert_eq!(get_long(&mut [0xFF; 11].as_slice()), Err(AvroError::Varint));
        assert_eq!(get_long(&mut [0x80].as_slice()), Err(AvroError::Truncated));
    }

    #[test]
    fn a_container_round_trips_and_is_the_same_bytes_twice() {
        let records: Vec<Value> = (0..4).map(value).collect();
        let meta = BTreeMap::from([("format-version".to_owned(), "3".to_owned())]);
        let bytes = write_container(&ty(), &meta, &records).unwrap();
        assert_eq!(bytes, write_container(&ty(), &meta, &records).unwrap());
        let (m, back) = read_container(&ty(), &bytes).unwrap();
        assert_eq!(m, meta);
        assert_eq!(back, records);
        // No records: a header and its sync marker only.
        let empty = write_container(&ty(), &meta, &[]).unwrap();
        assert_eq!(
            read_container(&ty(), &empty).unwrap().1,
            Vec::<Value>::new()
        );
        let schema = ty().to_json();
        assert_eq!(schema["fields"][7]["default"], Json::Null);
        assert_eq!(schema["fields"][6]["type"]["element-id"], 8);
        assert!(schema["fields"][0].get("default").is_none());
    }

    #[test]
    fn values_that_do_not_fit_are_refused() {
        let mut out = Vec::new();
        for (ty, v) in [
            (Ty::Long, Value::Int(1)),
            (
                Ty::Record {
                    name: "r",
                    fields: vec![],
                },
                Value::Record(vec![Value::Null]),
            ),
            (
                Ty::Union(vec![Ty::Null]),
                Value::Union(1, Box::new(Value::Null)),
            ),
            (
                Ty::Array {
                    items: Box::new(Ty::Long),
                    element_id: 1,
                },
                Value::Array(vec![Value::Null]),
            ),
        ] {
            assert!(
                matches!(encode(&ty, &v, &mut out), Err(AvroError::Mismatch { .. })),
                "{ty:?}"
            );
        }
        assert!(write_container(&Ty::Long, &BTreeMap::new(), &[Value::Null]).is_err());
        assert_eq!(
            AvroError::Mismatch {
                expected: Ty::Bytes.kind()
            }
            .to_string(),
            "a value does not fit the Avro schema: bytes expected"
        );
        for t in [
            Ty::Null,
            Ty::Boolean,
            Ty::Int,
            Ty::String,
            ty(),
            Ty::Union(vec![]),
            Ty::Array {
                items: Box::new(Ty::Null),
                element_id: 1,
            },
        ] {
            assert!(!t.kind().is_empty());
        }
    }

    #[test]
    fn broken_bytes_are_refused_with_the_reason() {
        let records: Vec<Value> = (0..3).map(value).collect();
        let bytes = write_container(&ty(), &BTreeMap::new(), &records).unwrap();
        assert_eq!(read_container(&ty(), b"Obj\x02"), Err(AvroError::Magic));
        assert_eq!(
            read_container(&Ty::Long, &bytes),
            Err(AvroError::Header("schema"))
        );
        // The last byte of the file is the end of the closing sync marker.
        let mut bad = bytes.clone();
        *bad.last_mut().unwrap() ^= 1;
        assert_eq!(read_container(&ty(), &bad), Err(AvroError::Sync));
        let cut = &bytes[..bytes.len() - 20];
        assert!(read_container(&ty(), cut).is_err());
        // A header that ends before its sync marker.
        let header_only = write_container(&ty(), &BTreeMap::new(), &[]).unwrap();
        assert_eq!(
            read_container(&ty(), &header_only[..header_only.len() - 1]),
            Err(AvroError::Truncated)
        );

        // Another codec.
        let mut h = MAGIC.to_vec();
        put_long(&mut h, 2);
        let schema = Ty::Long.to_json().to_string();
        for (k, v) in [("avro.codec", "deflate"), ("avro.schema", schema.as_str())] {
            put_bytes(&mut h, k.as_bytes());
            put_bytes(&mut h, v.as_bytes());
        }
        put_long(&mut h, 0);
        h.extend_from_slice(&[0; 16]);
        assert_eq!(
            read_container(&Ty::Long, &h),
            Err(AvroError::Header("codec"))
        );
        // A metadata block with a negative count and its size.
        let mut h = MAGIC.to_vec();
        put_long(&mut h, -1);
        put_long(&mut h, 99);
        put_bytes(&mut h, b"avro.schema");
        put_bytes(&mut h, schema.as_bytes());
        put_long(&mut h, 0);
        h.extend_from_slice(&[0; 16]);
        assert_eq!(read_container(&Ty::Long, &h), Ok((BTreeMap::new(), vec![])));
        // Metadata that is not UTF-8.
        let mut h = MAGIC.to_vec();
        put_long(&mut h, 1);
        put_bytes(&mut h, b"k");
        put_bytes(&mut h, &[0xFF]);
        put_long(&mut h, 0);
        assert_eq!(read_container(&Ty::Long, &h), Err(AvroError::Utf8));

        let mut s = Vec::new();
        put_bytes(&mut s, &[0xFF]);
        assert_eq!(decode(&Ty::String, &mut s.as_slice()), Err(AvroError::Utf8));
        assert_eq!(
            decode(&Ty::Bytes, &mut [10].as_slice()),
            Err(AvroError::Truncated)
        );
        assert_eq!(
            decode(&Ty::Bytes, &mut [1].as_slice()),
            Err(AvroError::Truncated)
        );
        assert_eq!(
            decode(&Ty::Boolean, &mut [].as_slice()),
            Err(AvroError::Truncated)
        );
        let mut big = Vec::new();
        put_long(&mut big, i64::from(i32::MAX) + 1);
        assert_eq!(
            decode(&Ty::Int, &mut big.as_slice()),
            Err(AvroError::Varint)
        );
        assert_eq!(
            decode(&Ty::Union(vec![Ty::Null]), &mut [4].as_slice()),
            Err(AvroError::Branch(2))
        );
        // A negative array count carries the block's size.
        let mut a = Vec::new();
        put_long(&mut a, -2);
        put_long(&mut a, 2);
        put_long(&mut a, 7);
        put_long(&mut a, 8);
        put_long(&mut a, 0);
        let longs = Ty::Array {
            items: Box::new(Ty::Long),
            element_id: 1,
        };
        assert_eq!(
            decode(&longs, &mut a.as_slice()),
            Ok(Value::Array(vec![Value::Long(7), Value::Long(8)]))
        );
        for e in [
            AvroError::Truncated,
            AvroError::Varint,
            AvroError::Utf8,
            AvroError::Branch(3),
            AvroError::Magic,
            AvroError::Header("codec"),
            AvroError::Sync,
        ] {
            assert!(e.to_string().contains("Avro"), "{e}");
        }
    }
}
