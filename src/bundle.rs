#![allow(dead_code)]

use std::{
    collections::HashMap,
    io::{Read as _, Seek as _},
};

use log::*;

use crate::{util, Error, FileInfo};

#[derive(Debug)]
pub struct AssetBundle {
    path: String,
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

    pub async fn get_uncompressed_info(&self) -> Result<HashMap<String, FileInfo>, Error> {
        let files = self.get_file_entries()?;
        let result = files
            .into_iter()
            .map(|file| (file.name, FileInfo::build_buffer(&file.data)))
            .collect();
        Ok(result)
    }

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
