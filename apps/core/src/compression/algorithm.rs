use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// Supported compression algorithms.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Algorithm {
    #[serde(rename = "7z")]
    SevenZ,
    #[serde(rename = "tar")]
    Tar,
    #[serde(rename = "zip")]
    Zip,
}

impl Algorithm {
    /// File extension for archives produced by this algorithm.
    pub fn extension(&self) -> &str {
        match self {
            Algorithm::SevenZ => "7z",
            Algorithm::Tar => "tar",
            Algorithm::Zip => "zip",
        }
    }

    /// MIME type for archives produced by this algorithm.
    pub fn media_type(&self) -> &str {
        match self {
            Algorithm::SevenZ => "application/x-7z-compressed",
            Algorithm::Tar => "application/x-tar",
            Algorithm::Zip => "application/zip",
        }
    }

    /// Try to detect the algorithm from a file extension.
    pub fn from_extension(ext: &str) -> Option<Self> {
        match ext.to_ascii_lowercase().as_str() {
            "7z" => Some(Algorithm::SevenZ),
            "tar" => Some(Algorithm::Tar),
            "zip" => Some(Algorithm::Zip),
            _ => None,
        }
    }
}

impl fmt::Display for Algorithm {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(self.extension())
    }
}

impl FromStr for Algorithm {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "7z" => Ok(Algorithm::SevenZ),
            "tar" => Ok(Algorithm::Tar),
            "zip" => Ok(Algorithm::Zip),
            other => Err(format!("Unknown algorithm: {other}")),
        }
    }
}
