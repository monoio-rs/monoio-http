use std::{
    collections::HashMap,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use bytes::{Bytes, BytesMut};
use http::HeaderMap;
use monoio::fs::File;

use crate::common::{
    body::{Body, HttpBody, StreamHint},
    error::{HttpError, ParseError},
};

pub const MAX_FILE_SIZE: u64 = 10 * 1024 * 1024; // 10 MB

#[derive(Debug, Clone)]
enum Data {
    InMemory(Bytes),
    InFile(PathBuf),
}

#[derive(Debug, Clone)]
pub struct FileHeader {
    filename: String,
    headers: HeaderMap,
    data: Data,
}

#[derive(Debug, Clone)]
pub struct FieldHeader {
    pub value: String,
    pub headers: HeaderMap,
}

#[derive(Debug)]
pub struct ParsedMultiPartForm {
    value: HashMap<String, Vec<FieldHeader>>,
    file: HashMap<String, Vec<FileHeader>>,
    boundary: String,
    first_part: bool,
    closed: bool,
    max_file_size: u64,
}

impl FieldHeader {
    pub fn get_value(&self) -> String {
        self.value.clone()
    }

    pub fn get_headers(&self) -> &HeaderMap {
        &self.headers
    }
}

impl FileHeader {
    pub fn get_filename(&self) -> String {
        self.filename.clone()
    }

    pub fn get_headers(&self) -> &HeaderMap {
        &self.headers
    }

    pub fn get_file_path(&self) -> Option<PathBuf> {
        match &self.data {
            Data::InMemory(_) => None,
            Data::InFile(file) => Some(file.clone()),
        }
    }
}

impl From<ParsedMultiPartForm> for HttpBody {
    fn from(p: ParsedMultiPartForm) -> Self {
        Self::Multipart(p)
    }
}

impl ParsedMultiPartForm {
    pub fn new(boundary: String, max_file_size: u64) -> Self {
        Self {
            value: HashMap::new(),
            file: HashMap::new(),
            boundary,
            first_part: true,
            closed: false,
            max_file_size,
        }
    }

    fn insert_field_value(&mut self, key: String, value: String, headers: HeaderMap) {
        self.value
            .entry(key)
            .or_default()
            .push(FieldHeader { value, headers });
    }

    fn insert_file(&mut self, key: String, file: FileHeader) {
        self.file.entry(key).or_default().push(file);
    }

    fn insert_file_data(&mut self, key: String, filename: String, headers: HeaderMap, data: Data) {
        self.insert_file(
            key,
            FileHeader {
                filename,
                headers,
                data,
            },
        );
    }

    pub fn get_field_value(&self, key: &str) -> Option<Vec<FieldHeader>> {
        self.value.get(key).cloned()
    }

    pub fn get_file(&self, key: &str) -> Option<Vec<FileHeader>> {
        self.file.get(key).map(|v| {
            v.iter()
                .map(|file| {
                    let data = match &file.data {
                        Data::InMemory(bytes) => Data::InMemory(bytes.clone()),
                        Data::InFile(file) => Data::InFile(file.clone()),
                    };
                    FileHeader {
                        filename: file.filename.clone(),
                        headers: file.headers.clone(),
                        data,
                    }
                })
                .collect()
        })
    }

    pub fn value_keys(&self) -> impl Iterator<Item = &String> {
        self.value.keys()
    }

    pub fn file_keys(&self) -> impl Iterator<Item = &String> {
        self.file.keys()
    }

    fn get_next_file_key(&self) -> Option<String> {
        self.file.keys().next().cloned()
    }

    fn write_part(&mut self, mut buf: BytesMut, headers: HeaderMap) -> Result<BytesMut, HttpError> {
        if !self.first_part {
            buf.extend_from_slice(format!("\r\n--{}\r\n", self.boundary).as_bytes());
        } else {
            buf.extend_from_slice(format!("--{}\r\n", self.boundary).as_bytes());
            self.first_part = false;
        }

        let mut keys = headers
            .keys()
            .map(|k| k.to_string())
            .collect::<Vec<String>>();
        keys.sort();

        for key in keys {
            let header_value = headers.get(&key.clone()).unwrap();
            match header_value.to_str() {
                Ok(value) => {
                    buf.extend_from_slice(format!("{}: {}\r\n", key, value).as_bytes());
                }
                Err(_) => {
                    buf.extend_from_slice(format!("{}: ", key).as_bytes());
                    buf.extend_from_slice(header_value.as_bytes());
                    buf.extend_from_slice("\r\n".to_string().as_bytes());
                }
            }
        }

        buf.extend_from_slice("\r\n".as_bytes());

        Ok(buf)
    }

    fn write_forms(&mut self) -> Result<Option<Bytes>, HttpError> {
        let mut buf = BytesMut::new();
        let cloned_value = self.value.clone(); // Clone self.value outside of the loop
        for (_, forms) in cloned_value.iter() {
            for form in forms {
                let headers = form.headers.clone(); // Store the value of form.headers in a separate variable
                buf = self.write_part(buf, headers.clone())?;
                buf.extend_from_slice(form.value.as_bytes());
            }
        }

        self.value.clear();

        Ok(Some(buf.freeze()))
    }

    async fn write_file(&mut self, key: String) -> Result<Option<Bytes>, HttpError> {
        let mut buf = BytesMut::new();
        for file in self.file.get(&key).unwrap().clone() {
            buf = self.write_part(buf, file.headers.clone())?;

            match &file.data {
                Data::InMemory(bytes) => {
                    buf.extend_from_slice(bytes.as_ref());
                }
                Data::InFile(file) => {
                    let file = File::open(file).await?;
                    let mut pos: u64 = 0;

                    loop {
                        let chunk = vec![0; 1024 * 1024]; // 1 KB chunk
                        let (res, read_chunk) = file.read_at(chunk, pos).await;
                        let bytes_written = res?;
                        pos += bytes_written as u64;
                        if bytes_written == 0 {
                            break;
                        }
                        buf.extend_from_slice(&read_chunk[..bytes_written]);
                    }
                }
            };
        }

        self.file.remove(&key);

        Ok(Some(buf.freeze()))
    }

    fn close_multipart(&mut self) -> Result<Option<Bytes>, HttpError> {
        let mut buf = BytesMut::new();
        self.closed = true;
        buf.extend_from_slice(format!("\r\n--{}--\r\n", self.boundary).as_bytes());
        Ok(Some(buf.freeze()))
    }

    pub async fn read_form(
        mut multer_multipart: multer::Multipart<'_>,
        boundary: String,
        max_file_size: u64,
    ) -> Result<Self, ParseError> {
        let mut form = ParsedMultiPartForm::new(boundary, max_file_size);
        while let Some(mut field) = multer_multipart.next_field().await? {
            let name = field.name().unwrap_or_default().to_string();
            let file_name = field.file_name().unwrap_or("").to_string();
            let headers = field.headers().clone();

            if file_name.is_empty() {
                let value = field.bytes().await?.to_vec();
                let value = String::from_utf8_lossy(&value).to_string();
                form.insert_field_value(name, value, headers);
                continue;
            }

            let timestamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs();

            let path = format!("/tmp/{}_{}.txt", file_name, timestamp);
            let file = monoio::fs::File::create(path.clone()).await?;
            let mut pos: u64 = 0;

            while let Some(bytes) = field.chunk().await? {
                let (res, _) = file.write_at(bytes, pos).await;
                pos += res? as u64;
            }

            let data = if pos < form.max_file_size {
                let _ = file.close().await;
                let buf = BytesMut::with_capacity(pos as usize);
                let file = monoio::fs::File::open(path.clone()).await?;
                let (res, ret_buf) = file.read_exact_at(buf, 0).await;
                res?;
                Data::InMemory(ret_buf.freeze())
            } else {
                Data::InFile(PathBuf::from(path))
            };

            form.insert_file_data(name, file_name, headers, data);
        }

        Ok(form)
    }
}

// Converts the ParsedMultiPartForm into a HttpBody stream by reconstructing the multipart form
// body. The values which are small in size are returned first, followed by the file data which is
// on the disk for every subsequent next_data call.
impl Body for ParsedMultiPartForm {
    type Data = Bytes;
    type Error = HttpError;

    async fn next_data(&mut self) -> Option<Result<Self::Data, Self::Error>> {
        if !self.value.is_empty() {
            return self
                .write_forms()
                .map_or_else(|e| Some(Err(e)), |v| v.map(Ok));
        }

        if let Some(key) = self.get_next_file_key() {
            return self
                .write_file(key)
                .await
                .map_or_else(|e| Some(Err(e)), |v| v.map(Ok));
        }

        if !self.closed && self.value.is_empty() && self.file.is_empty() {
            return self
                .close_multipart()
                .map_or_else(|e| Some(Err(e)), |v| v.map(Ok));
        }
        None
    }

    fn stream_hint(&self) -> StreamHint {
        StreamHint::Stream
    }
}
