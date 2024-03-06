use std::{collections::HashMap, io::{ Read, Write}, path::PathBuf };

use bytes::{Bytes, BytesMut};
use http::HeaderMap;
use tempfile::NamedTempFile;

use super::{body::{Body, HttpBody, StreamHint}, error::HttpError};

#[derive(Debug)]
enum  Data {
    InMemory(Bytes),
    InFile(NamedTempFile)
}

#[derive(Debug)]
pub struct FileHeader {
    filename: String,
    headers: HeaderMap,
    size: usize,
    data: Data 
}

#[derive(Debug)]
pub enum RetData {
    InMemory(Bytes),
    InFile(PathBuf)
}

#[derive(Debug)]
pub struct RetFileHeader {
    filename: String,
    headers: HeaderMap,
    data: RetData 
}

#[derive(Debug, Clone)]
pub struct FieldHeader {
    pub value: String,
    pub headers: HeaderMap
}


#[derive(Debug)]
pub struct ParsedMuliPartForm {
    value: HashMap<String, Vec<FieldHeader>>,
    file: HashMap<String, Vec<FileHeader>>,
    boundary: String,
    first_part: bool,
    closed: bool
}

impl FieldHeader {
    pub fn get_value(&self) -> String {
        self.value.clone()
    }

    pub fn get_headers(&self) -> &HeaderMap {
        &self.headers
    }
}

impl RetFileHeader {
    pub fn get_filename(&self) -> String {
        self.filename.clone()
    }

    pub fn get_headers(&self) -> &HeaderMap {
        &self.headers
    }

    pub fn get_data(&self) -> &RetData {
        &self.data
    }
}

impl From<ParsedMuliPartForm> for HttpBody {
    fn from(p: ParsedMuliPartForm) -> Self {
        Self::Multipart(p)
    }
}

impl ParsedMuliPartForm {
    pub fn new(boundary: String) -> Self {
        Self {
            value: HashMap::new(),
            file: HashMap::new(),
            boundary,
            first_part: true,
            closed: false,
        }
    }

    fn insert_field_value(&mut self, key: String,  value: String, headers: HeaderMap) {
        self.value.entry(key).or_insert_with(Vec::new).push(FieldHeader { value, headers });
    }

    fn insert_file(&mut self, key: String, file: FileHeader) {
        self.file.entry(key).or_insert_with(Vec::new).push(file);
    }

    fn insert_file_data(&mut self, key: String, filename: String, headers: HeaderMap, size: usize, data: Data) {
        self.insert_file(key, FileHeader { filename, headers, size, data });
    }

    pub fn  get_field_value(&self, key: &str) -> Option<Vec<FieldHeader>> {
        self.value.get(key).map(|v| v.clone())
    }

    pub fn get_file(&self, key: &str) -> Option<Vec<RetFileHeader>> {
        self.file.get(key).map(|v| {
            v.iter().map(|file| {
                let data = match &file.data {
                    Data::InMemory(bytes) => RetData::InMemory(bytes.clone()),
                    Data::InFile(file) => RetData::InFile(file.path().to_path_buf())
                };
                RetFileHeader {
                    filename: file.filename.clone(),
                    headers: file.headers.clone(),
                    data
                }
            }).collect()
        })
    }

    fn get_next_file(&mut self) -> Option<FileHeader> {
        for file in self.file.values_mut() {
            if let Some(file) = file.pop() {
                return Some(file);
            }
        }
        None
    }

    fn get_next_file_key(&self) -> Option<String> {
        for key in self.file.keys() {
            return Some(key.clone());
        }
        None
    }

    fn write_part(&self, mut buf: BytesMut, headers: HeaderMap) -> Result<BytesMut, HttpError> {

        if self.first_part == false {
            buf.extend_from_slice(format!("\r\n--{}\r\n", self.boundary).as_bytes());
        } else {
            buf.extend_from_slice(format!("--{}\r\n", self.boundary).as_bytes());
        }

        let mut keys = headers.keys().map(|k| k.to_string()).collect::<Vec<String>>();
        keys.sort();

        for key in keys {
            let value = headers.get(&key.clone()).unwrap();
            buf.extend_from_slice(format!("{}: {:?}\r\n", key, value).as_bytes());
        }

        buf.extend_from_slice("\r\n".as_bytes());

        Ok(buf)
    }

    fn write_forms(&mut self) -> Result<Option<Bytes>, HttpError> {
        let mut buf = BytesMut::new();
        for (_, forms) in self.value.iter() {
            for form in forms {
                let headers = form.headers.clone(); // Store the value of form.headers in a separate variable
                buf = self.write_part(buf, headers.clone())?;
                buf.extend_from_slice(form.value.as_bytes());
            } 
        }

        self.value.clear();

        Ok(Some(buf.freeze()))
    }

    fn write_file(&mut self, key: String) -> Result<Option<Bytes>, HttpError> {
        let mut buf = BytesMut::new();
        for file in self.file.get(&key).unwrap() {
            buf = self.write_part(buf,  file.headers.clone())?;

            match &file.data {
                Data::InMemory(bytes) => {
                    buf.extend_from_slice(bytes.as_ref());
                },
                Data::InFile(file) =>  {
                    let mut file = file.reopen()?; 
                    let mut chunk = vec![0; 1024 * 1024]; // 1 KB chunk
                    loop {
                        let n = file.read(&mut chunk)?;
                        if n == 0 {
                            break;
                        }
                        buf.extend_from_slice(&chunk[..n]);
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

    pub async fn read_form(mut multer_multipart: multer::Multipart<'_>, boundary: String) -> Result<Self, HttpError>{
        let mut form = ParsedMuliPartForm::new(boundary);
        while let Some(mut field) = multer_multipart.next_field().await? {

            let name = field.name().unwrap_or_default().to_string();
            let file_name = field.file_name().unwrap_or_else(|| "").to_string();
            let headers = field.headers().clone();

            if file_name == "" {
                let value = field.bytes().await?.to_vec();
                let value = String::from_utf8_lossy(&value).to_string();
                form.insert_field_value(name, value, headers);
                continue;
            }
            
            let mut buf = BytesMut::new();
            while let Some(bytes) = field.chunk().await? {
                buf.extend_from_slice(&bytes);
            }

            let MAX_FILE_SIZE = 10 * 1024 * 1024;
            let file_size = buf.len();

            let data = if  file_size > MAX_FILE_SIZE {
                let temp_file = tempfile::NamedTempFile::new().unwrap();
                let mut file = temp_file.reopen()?;
                file.write_all(&buf)?;
                file.flush()?;
                Data::InFile(temp_file)
            } else {
                Data::InMemory(buf.freeze())
            };
            form.insert_file_data(name, file_name, headers, file_size, data);
        }

        Ok(form)
    }
}

impl Body for ParsedMuliPartForm {
    type Data = Bytes;
    type Error = HttpError;

    async fn next_data(&mut self) ->  Option<Result<Self::Data, Self::Error>> {
        if self.value.len() > 0 {
            return self.write_forms().map_or_else(|e| Some(Err(e)), |v| v.map(|v| Ok(v)));
        }

        if let Some(key) = self.get_next_file_key() {
            return self.write_file(key).map_or_else(|e| Some(Err(e)), |v| v.map(|v| Ok(v)));;
        }

        if !self.closed && self.value.is_empty() && self.file.is_empty() {
            return self.close_multipart().map_or_else(|e| Some(Err(e)), |v| v.map(|v| Ok(v)));;
        }
        None
    }

    fn stream_hint(&self) -> StreamHint {
        StreamHint::Stream
    }
}

