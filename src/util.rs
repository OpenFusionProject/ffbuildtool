use std::io::{BufRead, Write as _};

use futures_util::StreamExt;
use log::*;
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt as _;
use uuid::Uuid;

use crate::{Error, ItemProgress, ProgressCallback};

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
        let permit = if let Some(permits) = crate::DOWNLOAD_PERMITS.get() {
            Some(permits.acquire().await.unwrap())
        } else {
            None
        };

        let response = reqwest::get(url).await?;
        let filename = Uuid::new_v4().to_string();
        let path = std::env::temp_dir().join(filename);
        let mut file = std::fs::File::create(&path)?;
        let bytes = response.bytes().await?;
        drop(permit);

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

pub async fn download_to_file(
    associated_uuid: Option<Uuid>,
    url: &str,
    file_path: &str,
    callback: Option<ProgressCallback>,
) -> Result<(), Error> {
    info!("Downloading {} to {}", url, file_path);

    let uuid = associated_uuid.unwrap_or(Uuid::nil());
    let file_name = get_file_name_without_parent(file_path);
    let mut file = tokio::fs::File::create(file_path).await?;

    if let Some(ref callback) = callback {
        callback(
            &uuid,
            file_name,
            ItemProgress::Downloading {
                bytes_downloaded: 0,
                total_bytes: 0,
            },
        );
    }

    // If the url is a file path, copy the file instead of downloading it
    if url.starts_with("file:///") {
        let path = url.trim_start_matches("file:///");
        let size = std::fs::metadata(path)?.len();
        if let Some(ref callback) = callback {
            callback(
                &uuid,
                file_name,
                ItemProgress::Downloading {
                    bytes_downloaded: 0,
                    total_bytes: size,
                },
            );
        }
        let reader = tokio::fs::read(path).await?;
        file.write_all(&reader).await?;
        if let Some(ref callback) = callback {
            callback(
                &uuid,
                file_name,
                ItemProgress::Downloading {
                    bytes_downloaded: size,
                    total_bytes: size,
                },
            );
        }
    } else {
        let _permit = if let Some(permits) = crate::DOWNLOAD_PERMITS.get() {
            Some(permits.acquire().await.unwrap())
        } else {
            None
        };

        let response = reqwest::get(url).await?;
        let total_size = response.content_length().unwrap_or(0);
        if let Some(ref callback) = callback {
            callback(
                &uuid,
                file_name,
                ItemProgress::Downloading {
                    bytes_downloaded: 0,
                    total_bytes: total_size,
                },
            );
        }

        let mut downloaded_size = 0;
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            file.write_all(&chunk).await?;
            downloaded_size += chunk.len() as u64;
            let progress = ItemProgress::Downloading {
                bytes_downloaded: downloaded_size,
                total_bytes: total_size,
            };
            if let Some(ref callback) = callback {
                callback(&uuid, file_name, progress);
            }
        }
    }
    Ok(())
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
