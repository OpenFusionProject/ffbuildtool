use std::{
    io::BufRead,
    sync::{Arc, Mutex},
};

use futures_util::StreamExt as _;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _, BufReader};
use uuid::Uuid;

use crate::{Error, FileInfo, ItemProgress, ProgressCallback};

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

pub fn get_file_name_from_url(url: &str) -> &str {
    let url = url.trim_end_matches('/');
    url.rsplit('/').next().unwrap_or(url)
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

/// RAII struct for temporary directories
pub struct TempDir {
    path: String,
}
impl Default for TempDir {
    fn default() -> Self {
        Self::new()
    }
}
impl TempDir {
    pub fn new() -> Self {
        let dir_name = Uuid::new_v4().to_string();
        let path = std::env::temp_dir().join(dir_name);
        std::fs::create_dir(&path).unwrap();
        Self {
            path: path.to_string_lossy().to_string(),
        }
    }

    pub fn path(&self) -> &str {
        &self.path
    }
}
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
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

    // Convert everything up to the first underscore to lowercase
    let first_underscore = output.find('_').unwrap_or(output.len());
    output[..first_underscore].make_ascii_lowercase();
    output
}

pub fn get_hash_as_string(hash: &[u8]) -> String {
    hash.iter().fold(String::new(), |mut s, byte| {
        s.push_str(&format!("{:02x}", byte));
        s
    })
}

const MAX_CHUNK_SIZE: usize = 8 * 1024;

async fn hash_chunk(chunk: [u8; MAX_CHUNK_SIZE], chunk_size: usize, hasher: Arc<Mutex<Sha256>>) {
    tokio::task::spawn_blocking(move || {
        let chunk = &chunk[..chunk_size];
        let mut hasher = hasher.lock().unwrap();
        hasher.update(chunk);
    })
    .await
    .unwrap();
}

pub async fn process_item_buffer(
    associated_uuid: Option<Uuid>,
    item_name: Option<&str>,
    mut buffer: Vec<u8>,
    out_file_path: Option<&str>,
    callback: Option<ProgressCallback>,
) -> Result<FileInfo, Error> {
    let item_name = item_name.unwrap_or("<buffer>");
    let uuid = associated_uuid.unwrap_or(Uuid::nil());
    let mut file = match out_file_path {
        Some(out_file_path) => {
            let file = tokio::fs::File::create(out_file_path).await?;
            Some(file)
        }
        None => None,
    };

    let total_size = buffer.len() as u64;
    if let Some(ref callback) = callback {
        callback(&uuid, item_name, ItemProgress::Downloading(0, total_size));
    }

    let hasher = Arc::new(Mutex::new(Sha256::new()));
    let mut bytes_processed = 0;
    let mut chunk_buffer = [0; MAX_CHUNK_SIZE];
    while !buffer.is_empty() {
        let chunk_size = std::cmp::min(buffer.len(), MAX_CHUNK_SIZE);
        chunk_buffer[..chunk_size].copy_from_slice(&buffer[..chunk_size]);
        buffer.drain(..chunk_size);
        bytes_processed += chunk_size as u64;
        if let Some(ref mut file) = file {
            file.write_all(&chunk_buffer[..chunk_size]).await?;
        }
        hash_chunk(chunk_buffer, chunk_size, hasher.clone()).await;
        if let Some(ref callback) = callback {
            callback(
                &uuid,
                item_name,
                ItemProgress::Downloading(bytes_processed, total_size),
            );
        }
    }

    assert!(bytes_processed == total_size);

    let hasher = Arc::try_unwrap(hasher).unwrap().into_inner().unwrap();
    let hash = hasher.finalize();
    Ok(FileInfo {
        size: bytes_processed,
        hash: get_hash_as_string(hash.as_slice()),
    })
}

pub async fn process_item(
    associated_uuid: Option<Uuid>,
    item_path: &str,
    out_file_path: Option<&str>,
    callback: Option<ProgressCallback>,
) -> Result<FileInfo, Error> {
    if item_path.starts_with("http://") || item_path.starts_with("https://") {
        process_item_http(associated_uuid, item_path, out_file_path, callback).await
    } else {
        process_item_file(associated_uuid, item_path, out_file_path, callback).await
    }
}

pub async fn process_item_file(
    associated_uuid: Option<Uuid>,
    file_path: &str,
    out_file_path: Option<&str>,
    callback: Option<ProgressCallback>,
) -> Result<FileInfo, Error> {
    let item_name = get_file_name_without_parent(file_path);
    let uuid = associated_uuid.unwrap_or(Uuid::nil());
    let mut file = match out_file_path {
        Some(out_file_path) => {
            let file = tokio::fs::File::create(out_file_path).await?;
            Some(file)
        }
        None => None,
    };

    let total_size = std::fs::metadata(file_path)?.len();
    if let Some(ref callback) = callback {
        callback(&uuid, item_name, ItemProgress::Downloading(0, total_size));
    }

    let hasher = Arc::new(Mutex::new(Sha256::new()));
    let in_file = tokio::fs::File::open(file_path).await?;
    let mut reader = BufReader::new(in_file);
    let mut bytes_processed = 0;
    let mut buffer = [0; MAX_CHUNK_SIZE];
    loop {
        let bytes_read = reader.read(&mut buffer).await?;
        if bytes_read == 0 {
            break;
        }
        bytes_processed += bytes_read as u64;
        if let Some(ref mut file) = file {
            file.write_all(&buffer[..bytes_read]).await?;
        }
        hash_chunk(buffer, bytes_read, hasher.clone()).await;
        if let Some(ref callback) = callback {
            callback(
                &uuid,
                item_name,
                ItemProgress::Downloading(bytes_processed, total_size),
            );
        }
    }

    if bytes_processed != total_size {
        return Err(format!(
            "Read size ({}) does not match source file size ({})",
            bytes_processed, total_size
        )
        .into());
    }

    let hasher = Arc::try_unwrap(hasher).unwrap().into_inner().unwrap();
    let hash = hasher.finalize();
    Ok(FileInfo {
        size: bytes_processed,
        hash: get_hash_as_string(hash.as_slice()),
    })
}

pub async fn process_item_http(
    associated_uuid: Option<Uuid>,
    url: &str,
    out_file_path: Option<&str>,
    callback: Option<ProgressCallback>,
) -> Result<FileInfo, Error> {
    let item_name = get_file_name_from_url(url);
    let uuid = associated_uuid.unwrap_or(Uuid::nil());
    let mut file = match out_file_path {
        Some(out_file_path) => {
            let file = tokio::fs::File::create(out_file_path).await?;
            Some(file)
        }
        None => None,
    };

    let response = reqwest::get(url)
        .await
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    let total_size = response.content_length().unwrap_or(0);
    if let Some(ref callback) = callback {
        callback(&uuid, item_name, ItemProgress::Downloading(0, total_size));
    }

    let hasher = Arc::new(Mutex::new(Sha256::new()));
    let mut stream = response.bytes_stream();
    let mut bytes_processed = 0;
    let mut buffer = [0; MAX_CHUNK_SIZE];
    while let Some(available) = stream.next().await {
        let mut available = available?.to_vec();
        while !available.is_empty() {
            let chunk_size = std::cmp::min(available.len(), MAX_CHUNK_SIZE);
            buffer[..chunk_size].copy_from_slice(&available[..chunk_size]);
            available.drain(..chunk_size);
            bytes_processed += chunk_size as u64;
            if let Some(ref mut file) = file {
                file.write_all(&buffer[..chunk_size]).await?;
            }
            hash_chunk(buffer, chunk_size, hasher.clone()).await;
            if let Some(ref callback) = callback {
                callback(
                    &uuid,
                    item_name,
                    ItemProgress::Downloading(bytes_processed, total_size),
                );
            }
        }
    }

    if total_size != 0 && bytes_processed != total_size {
        return Err(format!(
            "Downloaded size ({}) does not match expected size ({})",
            bytes_processed, total_size
        )
        .into());
    }

    let hasher = Arc::try_unwrap(hasher).unwrap().into_inner().unwrap();
    let hash = hasher.finalize();
    Ok(FileInfo {
        size: bytes_processed,
        hash: get_hash_as_string(hash.as_slice()),
    })
}

pub fn copy_dir(from: &str, to: &str, recursive: bool) -> Result<(), Error> {
    let from = std::path::Path::new(from);
    let to = std::path::Path::new(to);
    if from.is_dir() {
        if !from.exists() {
            std::fs::create_dir_all(to)?;
        }
        for entry in std::fs::read_dir(from)? {
            let entry = entry?;
            let path = entry.path();
            let new_path = to.join(path.file_name().unwrap());
            if path.is_dir() {
                if recursive {
                    copy_dir(&path.to_string_lossy(), &new_path.to_string_lossy(), true)?;
                }
            } else {
                std::fs::copy(&path, &new_path)?;
            }
        }
    } else {
        std::fs::copy(from, to)?;
    }
    Ok(())
}

pub fn file_path_to_uri(file_path: &str) -> String {
    let path = file_path.to_string();
    // Replace backslashes with forward slashes
    let path = path.replace("\\", "/");
    // Remove prefixed //?/ if it exists
    let path = path.trim_start_matches("//?/");
    // Add file:/// protocol
    format!("file:///{}", path)
}
