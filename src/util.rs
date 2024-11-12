use std::io::{BufRead, Write as _};

use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::Error;

pub fn get_file_hash(file_path: &str) -> Result<String, Error> {
    let file = std::fs::File::open(file_path)?;
    let mut reader = std::io::BufReader::new(file);
    let mut hasher = Sha256::new();
    std::io::copy(&mut reader, &mut hasher)?;
    Ok(format!("{:x}", hasher.finalize()))
}

pub fn get_buffer_hash(buffer: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(buffer);
    format!("{:x}", hasher.finalize())
}

pub fn get_file_extension(file_path: &str) -> Option<&str> {
    std::path::Path::new(file_path)
        .extension()
        .and_then(|ext| ext.to_str())
}

pub fn get_file_name_without_extension(file_path: &str) -> &str {
    std::path::Path::new(file_path)
        .file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or(file_path)
}

pub fn get_file_name_without_parent(file_path: &str) -> &str {
    std::path::Path::new(file_path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(file_path)
}

pub fn list_filenames_in_directory(directory_path: &str) -> Result<Vec<String>, Error> {
    let mut filenames = Vec::new();
    for entry in std::fs::read_dir(directory_path)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_file() {
            let name = path.file_name().unwrap().to_str().unwrap();
            filenames.push(name.to_string());
        }
    }
    Ok(filenames)
}

/// RAII struct for temporary files
pub struct TempFile {
    path: String,
}
impl TempFile {
    pub async fn download(url: &str) -> Result<Self, Error> {
        let response = reqwest::get(url).await?;
        let filename = Uuid::new_v4().to_string();
        let path = std::env::temp_dir().join(filename);
        let mut file = std::fs::File::create(&path)?;
        let bytes = response.bytes().await?;
        file.write_all(&bytes)?;
        Ok(Self {
            path: path.to_string_lossy().to_string(),
        })
    }

    pub fn path(&self) -> &str {
        &self.path
    }
}
impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

pub fn read_u32<T: BufRead>(reader: &mut T) -> Result<u32, Error> {
    let mut buf = [0; 4];
    reader.read_exact(&mut buf)?;
    let val = u32::from_be_bytes(buf);
    Ok(val)
}

pub fn read_u8<T: BufRead>(reader: &mut T) -> Result<u8, Error> {
    let mut buf = [0; 1];
    reader.read_exact(&mut buf)?;
    Ok(buf[0])
}

pub fn read_stringz<T: BufRead>(reader: &mut T) -> Result<String, Error> {
    let mut buf = Vec::new();
    reader.read_until(0, &mut buf)?;
    buf.pop(); // Remove the null terminator
    let string = String::from_utf8(buf)?;
    Ok(string)
}

#[cfg(feature = "lzma")]
pub fn decompress(data: &[u8]) -> Result<Vec<u8>, Error> {
    match lzma::decompress(data) {
        Ok(decompressed) => Ok(decompressed),
        Err(e) => Err(e.to_string().into()),
    }
}

pub fn url_encode(input: &str) -> String {
    let mut output = String::new();
    for byte in input.bytes() {
        if byte.is_ascii_alphanumeric() {
            output.push(byte as char);
        } else {
            output.push_str(&format!("_{:02x}", byte));
        }
    }
    output
}
