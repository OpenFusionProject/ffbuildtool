#![allow(dead_code)]

#[cfg(feature = "lzma")]
use std::{
    collections::HashMap,
    io::{Read as _, Seek as _},
    path::PathBuf,
};

use log::*;

use crate::{util, Error};

#[cfg(feature = "lzma")]
use crate::FileInfo;

#[derive(Debug)]
pub struct AssetBundle {
    path: String,
    //
    signature: String,
    format: u32,
    player_version: String,
    engine_version: String,
    file_size1: u32,
    data_offset: u32,
    unknown1: u32,
    offsets: Vec<FileOffsets>,
    file_size2: Option<u32>,
}

#[derive(Debug)]
struct FileOffsets {
    compressed_offset: u32,
    uncompressed_offset: u32,
}

#[derive(Debug)]
struct BundleFile {
    name: String,
    data: Vec<u8>,
}

impl AssetBundle {
    pub fn get_file_size(&self) -> u64 {
        // In the one case where file_size1 and file_size2 differ, I noticed
        // that file_size2 is the one thata matches the actual file size.
        self.file_size2.unwrap_or(self.file_size1) as u64
    }

    /// Returns the name of the file that this AssetBundle was created from.
    pub fn get_file_name(&self) -> &str {
        util::get_file_name_without_parent(&self.path)
    }

    pub fn from_file(path: &str) -> Result<Self, Error> {
        let mut file = std::fs::File::open(path)?;
        let mut reader = std::io::BufReader::new(&mut file);

        let signature = util::read_stringz(&mut reader)?;
        if signature != "UnityWeb" {
            return Err(format!("Invalid signature: {}, should be UnityWeb", signature).into());
        }

        let format = util::read_u32(&mut reader)?;
        if format != 2 {
            warn!("Unexpected format: {}, expected 2 (for {})", format, path);
        }

        let player_version = util::read_stringz(&mut reader)?;
        if player_version != "fusion-2.x.x" {
            warn!(
                "Unexpected player version: {}, expected fusion-2.x.x (for {})",
                player_version, path
            );
        }

        let engine_version = util::read_stringz(&mut reader)?;

        let file_size1 = util::read_u32(&mut reader)?;
        let data_offset = util::read_u32(&mut reader)?;
        let unknown1 = util::read_u32(&mut reader)?;
        let num_files = util::read_u32(&mut reader)?;

        assert!(unknown1 == num_files || unknown1 == 1);

        let mut offsets = Vec::new();
        for _ in 0..num_files {
            let compressed_offset = util::read_u32(&mut reader)?;
            let uncompressed_offset = util::read_u32(&mut reader)?;
            offsets.push(FileOffsets {
                compressed_offset,
                uncompressed_offset,
            });
        }

        let file_size2 = if format >= 2 {
            let size = util::read_u32(&mut reader)?;
            if size != file_size1 {
                warn!(
                    "File size 2 ({}) does not match file size 1 ({}) for {}",
                    size, file_size1, path
                );
            }
            Some(size)
        } else {
            None
        };

        Ok(Self {
            path: path.to_string(),
            signature,
            format,
            player_version,
            engine_version,
            file_size1,
            data_offset,
            unknown1,
            offsets,
            file_size2,
        })
    }

    #[cfg(feature = "lzma")]
    pub async fn get_uncompressed_info(&self) -> Result<HashMap<String, FileInfo>, Error> {
        use std::sync::Arc;

        use tokio::{sync::Mutex, task::JoinHandle};

        let files = self.get_file_entries()?;
        let info = Arc::new(Mutex::new(HashMap::new()));
        let mut tasks: Vec<JoinHandle<()>> = Vec::with_capacity(files.len());
        for file in files {
            let name = file.name.clone();
            let data = file.data.clone();
            let info = info.clone();
            tasks.push(tokio::spawn(async move {
                let file_info = util::process_item_buffer(None, Some(&name), data, None, None)
                    .await
                    .unwrap();
                info.lock().await.insert(name, file_info);
            }));
        }

        for task in tasks {
            task.await?;
        }

        let info = Arc::try_unwrap(info).unwrap().into_inner();
        Ok(info)
    }

    #[cfg(feature = "lzma")]
    pub fn extract_files(&self, output_dir: &str) -> Result<(), Error> {
        let url_encoded_name = util::url_encode(self.get_file_name());
        let path = PathBuf::from(output_dir).join(url_encoded_name);

        std::fs::create_dir_all(std::path::Path::new(&path))?;

        let files = self.get_file_entries()?;
        for file in files {
            let file_id = format!("{}{}", output_dir, file.name);
            let path = path.join(&file.name);
            if let Err(e) = std::fs::write(&path, &file.data) {
                warn!("Failed to write {}: {}", file_id, e);
            } else {
                info!("Extracted {}", file_id);
            }
        }
        Ok(())
    }

    #[cfg(feature = "lzma")]
    fn get_file_entries(&self) -> Result<Vec<BundleFile>, Error> {
        let mut file = std::fs::File::open(&self.path)?;
        let mut reader = std::io::BufReader::new(&mut file);
        reader.seek(std::io::SeekFrom::Start(self.data_offset as u64))?;
        let mut buf = Vec::new();
        reader.read_to_end(&mut buf)?;

        let uncompressed = util::decompress(&buf)?;
        let mut reader = std::io::BufReader::new(&uncompressed[..]);
        let num_files = util::read_u32(&mut reader)?;

        let mut files = Vec::with_capacity(num_files as usize);
        for _ in 0..num_files {
            let name = util::read_stringz(&mut reader)?;
            let offset = util::read_u32(&mut reader)?;
            let length = util::read_u32(&mut reader)?;
            let data = uncompressed[offset as usize..(offset + length) as usize].to_vec();
            let file = BundleFile { name, data };
            files.push(file);
        }

        Ok(files)
    }
}
