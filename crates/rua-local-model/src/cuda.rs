#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetectedOs {
    Linux,
    MacOs,
    Windows,
    Unknown,
}

impl DetectedOs {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Linux => "linux",
            Self::MacOs => "macos",
            Self::Windows => "windows",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetectedArch {
    X86_64,
    Aarch64,
    Unknown,
}

impl DetectedArch {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::X86_64 => "x86_64",
            Self::Aarch64 => "aarch64",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CudaInfo {
    pub available: bool,
    pub version: Option<String>,
    pub vram_mb: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct SystemInfo {
    pub os: DetectedOs,
    pub arch: DetectedArch,
    pub ram_mb: u64,
    pub cuda: CudaInfo,
}

pub fn detect_os() -> DetectedOs {
    if cfg!(target_os = "linux") {
        DetectedOs::Linux
    } else if cfg!(target_os = "macos") {
        DetectedOs::MacOs
    } else if cfg!(target_os = "windows") {
        DetectedOs::Windows
    } else {
        DetectedOs::Unknown
    }
}

pub fn detect_arch() -> DetectedArch {
    if cfg!(target_arch = "x86_64") {
        DetectedArch::X86_64
    } else if cfg!(target_arch = "aarch64") {
        DetectedArch::Aarch64
    } else {
        DetectedArch::Unknown
    }
}

pub fn detect_cuda() -> CudaInfo {
    let output = std::process::Command::new("nvidia-smi")
        .arg("--query-gpu=driver_version,memory.total")
        .arg("--format=csv,noheader,nounits")
        .output();

    match output {
        Ok(out) if out.status.success() => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let line = stdout.lines().next().unwrap_or("");
            let parts: Vec<&str> = line.split(',').map(|s| s.trim()).collect();
            let version = parts.first().map(|s| s.to_string());
            let vram_mb = parts.get(1).and_then(|s| s.parse::<u64>().ok());
            CudaInfo {
                available: true,
                version,
                vram_mb,
            }
        }
        _ => CudaInfo::default(),
    }
}

pub fn detect_ram_mb() -> u64 {
    #[cfg(target_os = "linux")]
    {
        if let Ok(contents) = std::fs::read_to_string("/proc/meminfo") {
            for line in contents.lines() {
                if line.starts_with("MemTotal:") {
                    let kb: u64 = line
                        .split_whitespace()
                        .nth(1)
                        .and_then(|s| s.parse().ok())
                        .unwrap_or(0);
                    return kb / 1024;
                }
            }
        }
        0
    }
    #[cfg(target_os = "macos")]
    {
        let output = std::process::Command::new("sysctl")
            .arg("-n")
            .arg("hw.memsize")
            .output();
        match output {
            Ok(out) if out.status.success() => {
                let bytes: u64 = String::from_utf8_lossy(&out.stdout)
                    .trim()
                    .parse()
                    .unwrap_or(0);
                bytes / (1024 * 1024)
            }
            _ => 0,
        }
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        0
    }
}

pub fn detect_system_info() -> SystemInfo {
    SystemInfo {
        os: detect_os(),
        arch: detect_arch(),
        ram_mb: detect_ram_mb(),
        cuda: detect_cuda(),
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RecommendedModel {
    pub id: String,
    pub name: String,
    pub size_bytes: u64,
    pub downloaded: bool,
    pub source: String,
}

pub fn get_recommended_models(memory_gb: u64) -> Vec<RecommendedModel> {
    if memory_gb < 4 {
        return Vec::new();
    }

    if memory_gb <= 8 {
        vec![
            RecommendedModel {
                id: "AgentScope/Rua-Flash-2B-Q4_K_M".into(),
                name: "Rua-Flash-2B-Q4_K_M".into(),
                size_bytes: 1_560_460_768,
                downloaded: false,
                source: "modelscope".into(),
            },
            RecommendedModel {
                id: "AgentScope/Rua-Flash-2B-Q8_0".into(),
                name: "Rua-Flash-2B-Q8_0".into(),
                size_bytes: 2_552_356_320,
                downloaded: false,
                source: "modelscope".into(),
            },
        ]
    } else if memory_gb <= 16 {
        vec![
            RecommendedModel {
                id: "AgentScope/Rua-Flash-4B-Q4_K_M".into(),
                name: "Rua-Flash-4B-Q4_K_M".into(),
                size_bytes: 3_066_384_736,
                downloaded: false,
                source: "modelscope".into(),
            },
            RecommendedModel {
                id: "AgentScope/Rua-Flash-4B-Q8_0".into(),
                name: "Rua-Flash-4B-Q8_0".into(),
                size_bytes: 5_157_833_056,
                downloaded: false,
                source: "modelscope".into(),
            },
        ]
    } else {
        vec![
            RecommendedModel {
                id: "AgentScope/Rua-Flash-9B-Q4_K_M".into(),
                name: "Rua-Flash-9B-Q4_K_M".into(),
                size_bytes: 5_476_080_128,
                downloaded: false,
                source: "modelscope".into(),
            },
            RecommendedModel {
                id: "AgentScope/Rua-Flash-9B-Q8_0".into(),
                name: "Rua-Flash-9B-Q8_0".into(),
                size_bytes: 10_590_617_600,
                downloaded: false,
                source: "modelscope".into(),
            },
        ]
    }
}
