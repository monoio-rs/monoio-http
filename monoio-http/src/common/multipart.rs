use std::{collections::HashMap, io::{Cursor, Read, Write} };

use bytes::{Bytes, BytesMut};
use http::HeaderMap;
use tempfile::NamedTempFile;

use super::error::HttpError;

enum  Data {
    InMemory(Cursor<Bytes>),
    InFile(NamedTempFile)
}

pub struct FileHeader {
    filename: String,
    headers: HeaderMap,
    size: usize,
    data: Data 
}

pub struct ParsedMuliPartForm {
    value: HashMap<String, Vec<String>>,
    file: HashMap<String, Vec<FileHeader>>,
}

impl Read for Data {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Data::InMemory(cursor) => cursor.read(buf),
            Data::InFile(file) => file.read(buf),
        }
    }
}

impl ParsedMuliPartForm {
    pub fn new() -> Self {
        Self {
            value: HashMap::new(),
            file: HashMap::new(),
        }
    }

    fn insert_value(&mut self, key: String, value: String) {
        self.value.entry(key).or_insert_with(Vec::new).push(value);
    }

    fn insert_file(&mut self, key: String, file: FileHeader) {
        self.file.entry(key).or_insert_with(Vec::new).push(file);
    }

    fn insert_file_data(&mut self, key: String, filename: String, headers: HeaderMap, size: usize, data: Data) {
        self.insert_file(key, FileHeader { filename, headers, size, data });
    }

    async fn read_form(mut multer_multipart: multer::Multipart<'_>) -> Result<Self, HttpError>{
        let mut form = ParsedMuliPartForm::new();
        while let Some(mut field) = multer_multipart.next_field().await? {

            let name = field.name().unwrap_or_default().to_string();
            let file_name = field.file_name().unwrap_or_else(|| "").to_string();
            let headers = field.headers().clone();

            if file_name == "" {
                let value = field.bytes().await?.to_vec();
                let value = String::from_utf8_lossy(&value).to_string();
                form.insert_value(name, value);
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
                let cursor = Cursor::new(buf.freeze());
                Data::InMemory(cursor)
            };
            form.insert_file_data(name, file_name, headers, file_size, data);
        }

        Ok(form)
    }

}



