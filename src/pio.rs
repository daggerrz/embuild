use std::{collections::{HashMap, HashSet}, convert::{TryFrom, TryInto}, fs::File, io::{Read, Write}, path::{Path, PathBuf}, process::{Command, Output}};

use anyhow::*;
use log::*;

use tempfile::*;

use serde::{Deserialize, Serialize, de::DeserializeOwned};

pub mod cargo;

const INSTALLER_URL: &str = "https://raw.githubusercontent.com/platformio/platformio-core-installer/master/get-platformio.py";
const INSTALLER_BLOB: &[u8] = include_bytes!("get-platformio.py");

#[derive(Serialize, Deserialize, Default, Clone, Debug)]
pub struct Framework {
    pub name: String,
    pub title: String,
    pub description: String,
    pub url: String,
    pub homepage: String,
    pub platforms: Vec<String>,
}

#[derive(Serialize, Deserialize, Default, Clone, Debug)]
pub struct LibrariesPage {
    pub page: u32,
    pub perpage: u32,
    pub total: u32,
    #[serde(default)]
    pub items: Vec<Library>,
}

#[derive(Serialize, Deserialize, Default, Clone, Debug)]
pub struct Library {
    pub id: u32,
    pub name: String,
    pub description: String,
    pub updated: String,
    pub dllifetime: u64,
    pub dlmonth: u64,
    pub examplenums: u32,
    pub versionname: String,
    pub ownername: String,
    #[serde(default)]
    pub authornames: Vec<String>,
    #[serde(default)]
    pub keywords: Vec<String>,
    #[serde(default)]
    pub frameworks: Vec<LibraryFrameworkOrPlatformRef>,
    #[serde(default)]
    pub platforms: Vec<LibraryFrameworkOrPlatformRef>,
}

#[derive(Serialize, Deserialize, Default, Clone, Debug)]
pub struct LibraryFrameworkOrPlatformRef {
    pub name: String,
    pub title: String,
}

#[derive(Serialize, Deserialize, Default, Clone, Debug)]
pub struct Board {
    pub id: String,
    pub name: String,
    pub platform: String,
    pub mcu: String,
    pub fcpu: u64,
    pub ram: u64,
    pub rom: u64,
    pub frameworks: Vec<String>,
    pub vendor: String,
    pub url: String,
    #[serde(default)]
    pub connectivity: Vec<String>,
    #[serde(default)]
    pub debug: BoardDebug,
}

#[derive(Serialize, Deserialize, Default, Clone, Debug)]
pub struct BoardDebug { #[serde(default)] pub tools: HashMap<String, HashMap<String, bool>> }

#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct Pio {
    pub is_develop_core: bool,
    pub platformio_exe: PathBuf,
    pub penv_dir: PathBuf,
    pub installer_version: String,
    pub python_version: String,
    pub core_version: String,
    pub system: String,
    pub python_exe: PathBuf,
    pub cache_dir: PathBuf,
    pub penv_bin_dir: PathBuf,
    pub core_dir: PathBuf,
}

impl Pio {
    pub fn cmd(&self) -> Command {
        let mut command = Command::new(&self.platformio_exe);

        command.env("PLATFORMIO_CORE_DIR", &self.core_dir);

        command
    }

    pub fn project(&self, project: &Path) -> Command {
        let mut command = self.cmd();

        command.current_dir(project);

        command
    }

    pub fn check(output: &Output) -> Result<()> {
        if !output.status.success() {
            bail!("PIO returned status code {:?} and error stream {}", output.status.code(), String::from_utf8(output.stderr.clone())?);
        }

        Ok(())
    }

    pub fn json<T: DeserializeOwned>(cmd: &mut Command) -> Result<T> {
        cmd.arg("--json-output");
        debug!("Running command {:?}", cmd);

        let output = cmd.output()?;

        Self::check(&output)?;

        Ok(serde_json::from_slice::<T>(&output.stdout)?)
    }

    pub fn boards(&self, id: Option<impl AsRef<str>>) -> Result<Vec<Board>> {
        let mut cmd = self.cmd();

        cmd.arg("boards");

        if let Some(search_str) = id.as_ref() {
            cmd.arg(search_str.as_ref());
        }

        let result = Self::json::<Vec<Board>>(&mut cmd);

        if let Some(search_str) = id {
            Ok(result?.into_iter().filter(|b| b.id == search_str.as_ref()).collect::<Vec<_>>())
        } else {
            result
        }
    }

    pub fn library(&self, name: Option<impl AsRef<str>>) -> Result<Library> {
        let mut cmd = self.cmd();

        cmd.arg("lib").arg("show");

        if let Some(name) = name {
            cmd.arg("--name").arg(name.as_ref());
        }

        Self::json::<Library>(&mut cmd)
    }

    pub fn libraries<S: AsRef<str>>(&self, names: &[S]) -> Result<Vec<Library>> {
        let mut res = Vec::<Library>::new();

        loop {
            let mut cmd = self.cmd();

            cmd.arg("lib").arg("search");

            for name in names {
                cmd.arg("--name").arg(name.as_ref());
            }

            let page = Self::json::<LibrariesPage>(&mut cmd)?;

            for library in page.items {
                res.push(library);
            }

            if page.page == page.total {
                break Ok(res)
            }
        }
    }

    pub fn frameworks(&self, name: Option<impl AsRef<str>>) -> Result<Vec<Framework>> {
        let mut cmd = self.cmd();

        cmd.arg("platform").arg("frameworks");

        if let Some(search_str) = name.as_ref() {
            cmd.arg(search_str.as_ref());
        }

        let result = Self::json::<Vec<Framework>>(&mut cmd);

        if let Some(search_str) = name {
            Ok(result?.into_iter().filter(|f| f.name == search_str.as_ref()).collect::<Vec<_>>())
        } else {
            result
        }
    }
}

#[derive(Debug)]
pub struct PioInstaller {
    installer_location: PathBuf,
    installer_temp: Option<TempPath>,
    pio_location: Option<PathBuf>,
}

impl<T: Into<PathBuf>> From<T> for PioInstaller {
    fn from(path: T) -> Self {
        Self {
            installer_location: path.into(),
            installer_temp: None,
            pio_location: None,
        }
    }
}

impl PioInstaller {
    pub fn new() -> Result<Self> {
        Self::create(false)
    }

    pub fn new_download() -> Result<Self> {
        Self::create(true)
    }

    pub fn new_location(installer_location: impl Into<PathBuf>) -> Self {
        PioInstaller::from(installer_location)
    }

    fn create(download: bool) -> Result<Self> {
        let mut file = NamedTempFile::new()?;

        let writer = file.as_file_mut();

        if download {
            debug!("Downloading get-platformio.py from {}", INSTALLER_URL);

            let mut reader = ureq::get(INSTALLER_URL)
                .call()?
                .into_reader();

            let mut buffer = [0 as u8; 4096];

            loop {
                let len = reader.read(&mut buffer)?;
                if len == 0 {
                    break;
                }

                writer.write(&buffer[0..len])?;
            }
        } else {
            debug!("Using built-in get-platformio.py");

            writer.write(INSTALLER_BLOB)?;
        }

        let temp_path = file.into_temp_path();

        Ok(Self {
            installer_location: temp_path.to_path_buf(),
            installer_temp: Some(temp_path),
            pio_location: None,
        })
    }

    pub fn pio(&mut self, pio_location: impl Into<PathBuf>) -> &mut Self {
        let pio_location = pio_location.into();

        debug!("Using PlatformIO installation {}", pio_location.display());

        self.pio_location = Some(pio_location);
        self
    }

    pub fn update(&self) -> Result<Pio> {
        if let Ok(pio) = self.check() {
            info!("PlatformIO is up-to-date");

            Ok(pio)
        } else {
            info!("PlatformIO needs to be installed or updated");

            self.install()?;
            Ok(self.check()?)
        }
    }

    pub fn install(&self) -> Result<()> {
        let mut cmd = self.command();

        debug!("Running command {:?}", cmd);

        cmd.status()?;

        Ok(())
    }

    pub fn check(&self) -> Result<Pio> {
        let (file, path) = NamedTempFile::new()?.into_parts();

        let mut cmd = self.command();

        cmd
            .arg("check")
            .arg("core")
            .arg("--dump-state")
            .arg(&path);

        debug!("Running command {:?}", cmd);

        cmd.status()?;

        Ok(serde_json::from_reader::<File, Pio>(file)?)
    }

    fn command(&self) -> Command {
        let mut command = Command::new("python");

        if let Some(pio_location) = self.pio_location.as_ref() {
            command.env("PLATFORMIO_CORE_DIR", pio_location);
        }

        command.arg(&self.installer_location);

        command
    }
}

#[derive(Clone, Debug)]
pub struct Resolver {
    pio: Pio,
    target: Option<String>,
    params: ResolutionParams,
}

#[derive(Clone, Debug, Default)]
pub struct ResolutionParams {
    pub board: Option<String>,
    pub mcu: Option<String>,
    pub platform: Option<String>,
    pub frameworks: Vec<String>,
}

impl TryFrom<ResolutionParams> for Resolution {
    type Error = anyhow::Error;

    fn try_from(params: ResolutionParams) -> Result<Self, Self::Error> {
        if let Some(board) = params.board {
            if let Some(mcu) = params.mcu {
                if let Some(platform) = params.platform {
                    if !params.frameworks.is_empty() {
                        return Ok(Self {
                            board,
                            mcu,
                            platform,
                            frameworks: params.frameworks.clone(),
                        });
                    }
                }
            }
        }

        bail!("Error");
    }
}

struct TargetConf {
    platform: &'static str,
    mcu: &'static str,
    frameworks: Vec<&'static str>,
}

#[derive(Clone, Debug, Default)]
pub struct Resolution {
    pub board: String,
    pub mcu: String,
    pub platform: String,
    pub frameworks: Vec<String>,
}

impl Resolver {
    pub fn new(pio: Pio) -> Self {
        Self {
            pio,
            target: None,
            params: Default::default(),
        }
    }

    pub fn params(mut self, params: ResolutionParams) -> Self {
        self.params = params;

        self
    }

    pub fn board(mut self, board: impl Into<String>) -> Self {
        self.params.board = Some(board.into());

        self
    }

    pub fn mcu(mut self, mcu: impl Into<String>) -> Self {
        self.params.mcu = Some(mcu.into());

        self
    }

    pub fn platform(mut self, platform: impl Into<String>) -> Self {
        self.params.platform = Some(platform.into());

        self
    }

    pub fn frameworks(mut self, frameworks: Vec<String>) -> Self {
        self.params.frameworks = frameworks;

        self
    }

    pub fn target(mut self, target: impl Into<String>) -> Self {
        self.target = Some(target.into());

        self
    }

    pub fn resolve(&self) -> Result<Resolution> {
        let resolution = if self.params.board.is_some() {
            self.resolve_platform_by_board()?
        } else {
            self.resolve_platform_all()?
        };

        info!(
            "Resolved platform: '{}', MCU: '{}', board: '{}', frameworks: [{}]",
            resolution.platform,
            resolution.mcu,
            resolution.board,
            resolution.frameworks.join(", "));

        Ok(resolution)
    }

    fn resolve_platform_by_board(&self) -> Result<Resolution> {
        let mut params = self.params.clone();

        let board_id = params.board.as_ref().unwrap().as_str();

        let boards: Vec<Board> = self.pio.boards(None as Option<String>)?
            .into_iter()
            .filter(|b| b.id == board_id)
            .collect::<Vec<_>>();

        if boards.is_empty() {
            bail!("Configured board '{}' is not known to PIO", board_id);
        }

        if boards.len() > 1 {
            bail!(
                "Configured board '{}' matches multiple boards in PIO: [{}]",
                board_id,
                boards.iter().map(|b| b.id.as_str()).collect::<Vec<_>>().join(", "));
        }

        let board = &boards[0];

        let target_pmf = self.get_default_platform_mcu_frameworks().ok();
        if let Some(target_pmf) = target_pmf {
            let target = self.target.as_ref().unwrap();

            if board.platform != target_pmf.platform {
                bail!(
                    "Platforms mismatch: configured board '{}' has platform '{}' in PIO, which does not match platform '{}' derived from the build target '{}'",
                    board.id,
                    board.platform,
                    target_pmf.platform,
                    target);
            }

            if board.mcu != target_pmf.mcu {
                bail!(
                    "MCUs mismatch: configured board '{}' has MCU '{}' in PIO, which does not match MCU '{}' derived from the build target '{}'",
                    board.id,
                    board.mcu,
                    target_pmf.mcu,
                    target);
            }

            if target_pmf.frameworks.iter().find(|f| board.frameworks.iter().find(|f2| **f == f2.as_str()).is_none()).is_some() {
                bail!(
                    "Frameworks mismatch: configured board '{}' has frameworks [{}] in PIO, which do not contain the frameworks [{}] derived from the build target '{}'",
                    board.id,
                    board.frameworks.join(", "),
                    target_pmf.frameworks.join(", "),
                    target);
            }

            if params.platform.is_none() {
                info!(
                    "Configuring platform '{}' derived from the build target '{}'",
                    target_pmf.platform,
                    self.target.as_ref().unwrap());

                params.platform = Some(target_pmf.platform.into());
            }

            if params.mcu.is_none() {
                info!(
                    "Configuring MCU '{}' derived from the build target '{}'",
                    target_pmf.mcu,
                    target);

                params.mcu = Some(target_pmf.mcu.into());
            }

            if params.frameworks.is_empty() {
                info!(
                    "Configuring framework '{}' from the frameworks [{}] derived from the build target '{}'",
                    target_pmf.frameworks[0],
                    target_pmf.frameworks.join(", "),
                    target);

                params.frameworks.push(target_pmf.frameworks[0].into());
            }
        }

        if let Some(configured_platform) = params.platform.as_ref() {
            if *configured_platform != board.platform {
                bail!(
                    "Platforms mismatch: configured board '{}' has platform '{}' in PIO, which does not match the configured platform '{}'",
                    board.id,
                    board.platform,
                    configured_platform);
            }
        } else {
            info!(
                "Configuring platform '{}' supported by the configured board '{}'",
                board.platform,
                board.id);

            params.platform = Some(board.platform.clone());
        }

        if let Some(configured_mcu) = params.mcu.as_ref() {
            if *configured_mcu != board.mcu {
                bail!(
                    "Platforms mismatch: configured board '{}' has MCU '{}' in PIO, which does not match the configured MCU '{}'",
                    board.id,
                    board.mcu,
                    configured_mcu);
            }
        } else {
            info!(
                "Configuring MCU '{}' supported by the configured board '{}'",
                board.mcu,
                board.id);

            params.mcu = Some(board.mcu.clone());
        }

        if !params.frameworks.is_empty() {
            if params.frameworks.iter().find(|f| board.frameworks.iter().find(|f2| f2.as_str() == f.as_str()).is_none()).is_some() {
                bail!(
                    "Frameworks mismatch: configured board '{}' has frameworks [{}] in PIO, which do not contain the configured frameworks [{}]",
                    board.id,
                    board.frameworks.join(", "),
                    params.frameworks.join(", "));
            }
        } else {
            info!(
                "Configuring framework '{}' from the frameworks [{}] supported by the configured board '{}'",
                board.frameworks[0],
                board.frameworks.join(", "),
                board.id);

            params.frameworks.push(board.frameworks[0].clone());
        }

        params.try_into()
    }

    fn resolve_platform_all(&self) -> Result<Resolution> {
        let mut params = self.params.clone();

        let target_pmf = self.get_default_platform_mcu_frameworks().ok();
        if let Some(target_pmf) = target_pmf {
            let target = self.target.as_ref().unwrap();

            if let Some(configured_platform) = params.platform.as_ref() {
                if configured_platform != target_pmf.platform {
                    bail!(
                        "Platforms mismatch: configured platform '{}' does not match platform '{}', which was derived from the build target '{}'",
                        configured_platform,
                        target_pmf.platform,
                        target);
                    }
            } else {
                info!(
                    "Configuring platform '{}' derived from the build target '{}'",
                    target_pmf.platform,
                    target);

                params.platform = Some(target_pmf.platform.into());
            }

            if let Some(configured_mcu) = params.mcu.as_ref() {
                if configured_mcu != target_pmf.mcu {
                    bail!(
                        "MCUs mismatch: configured MCU '{}' does not match MCU '{}', which was derived from the build target '{}'",
                        configured_mcu,
                        target_pmf.mcu,
                        target);
                }
            } else {
                info!(
                    "Configuring MCU '{}' derived from the build target '{}'",
                    target_pmf.mcu,
                    target);

                    params.mcu = Some(target_pmf.mcu.into());
            }

            if !params.frameworks.is_empty() {
                if target_pmf.frameworks.iter().find(|f| params.frameworks.iter().find(|f2| f2.as_str() == **f).is_some()).is_none() {
                    bail!(
                        "Frameworks mismatch: configured frameworks [{}] are not contained in the frameworks [{}], which were derived from the build target '{}'",
                        params.frameworks.join(", "),
                        target_pmf.frameworks.join(", "),
                        target);
                }
            } else {
                info!(
                    "Configuring framework '{}' from the frameworks [{}] derived from the build target '{}'",
                    target_pmf.frameworks[0],
                    target_pmf.frameworks.join(", "),
                    target);

                    params.frameworks.push(target_pmf.frameworks[0].into());
            }
        }

        let mut frameworks = self.pio.frameworks(None as Option<String>)?;

        if !params.frameworks.is_empty() {
            let not_found_frameworks = params.frameworks
                .iter()
                .filter(|f| frameworks.iter().find(|f2| f2.name == f.as_str()).is_none())
                .map(|s| s.as_str())
                .collect::<Vec<_>>();

            if !not_found_frameworks.is_empty() {
                bail!("(Some of) the configured frameworks [{}] are not known to PIO", not_found_frameworks.join(", "));
            }
        }

        if let Some(configured_platform) = params.platform.as_ref() {
            let frameworks_for_platform = frameworks
                .clone()
                .into_iter()
                .filter(|f| f.platforms.iter().find(|p| p.as_str() == configured_platform).is_some())
                .collect::<Vec<_>>();

            if frameworks_for_platform.is_empty() {
                bail!("Configured platform '{}' is not known to PIO", configured_platform);
            }

            frameworks = frameworks_for_platform;

            if !params.frameworks.is_empty() {
                let not_found_frameworks = params.frameworks
                    .iter()
                    .filter(|f| frameworks.iter().find(|f2| f2.name == f.as_str()).is_none())
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>();

                if !not_found_frameworks.is_empty() {
                    bail!(
                        "(Some of) the configured frameworks [{}] are not supported by the configured platform '{}'",
                        not_found_frameworks.join(", "),
                        configured_platform);
                }
            } else {
                info!(
                    "Configuring framework '{}' from the frameworks [{}] matching the configured platform '{}'",
                    frameworks[0].name,
                    frameworks.iter().map(|f| f.name.as_str()).collect::<Vec<_>>().join(", "),
                    configured_platform);

                params.frameworks.push(frameworks[0].name.clone());
            }
        } else {
            let platforms = frameworks.into_iter()
                .filter(|f| params.frameworks.iter().find(|f2| f.name == f2.as_str()).is_some())
                .map(|f| f.platforms)
                .reduce(|s1, s2|
                    s1.into_iter().collect::<HashSet<_>>()
                        .intersection(&s2.into_iter().collect::<HashSet<_>>())
                        .map(|s| s.clone())
                        .collect::<Vec<_>>())
                .unwrap_or(Vec::new());

            if platforms.is_empty() {
                bail!("Cannot select a platform: configured frameworks [{}] do not have a common platform", params.frameworks.join(", "));
            }

            if platforms.len() > 1 {
                bail!(
                    "Cannot select a platform: configured frameworks [{}] have multiple common platforms: [{}]",
                    params.frameworks.join(", "),
                    platforms.join(", "));
            }

            info!(
                "Configuring platform '{}' as the only common one of the configured frameworks [{}]",
                platforms[0],
                params.frameworks.join(", "));

                params.platform = Some(platforms[0].clone());
        }

        let mut boards = self.pio.boards(None as Option<String>)?
            .into_iter()
            .filter(|b|
                b.platform == *params.platform.as_ref().unwrap()
                && params.frameworks.iter().find(|f| b.frameworks.iter().find(|f2| f.as_str() == f2.as_str()).is_none()).is_none())
            .collect::<Vec<_>>();

        trace!(
            "Boards supporting configured platform '{}' and configured frameworks [{}]: [{}]",
            params.platform.as_ref().unwrap(),
            params.frameworks.join(", "),
            boards.iter().map(|b| b.id.as_str()).collect::<Vec<_>>().join(", "));

        if boards.is_empty() {
            bail!(
                "Configured platform '{}' and frameworks [{}] do not have any matching board defined in PIO",
                params.platform.as_ref().unwrap(),
                params.frameworks.join(", "));
        } else {
            if params.mcu.is_some() {
                boards = boards
                    .into_iter()
                    .filter(|b| b.mcu == *params.mcu.as_ref().unwrap())
                    .collect::<Vec<_>>();

                if boards.is_empty() {
                    bail!(
                        "Configured platform '{}', MCU '{}' and frameworks [{}] do not have any matching board defined in PIO",
                        params.platform.as_ref().unwrap(),
                        params.mcu.as_ref().unwrap(),
                        params.frameworks.join(", "));
                }
            } else {
                let mcus = boards.iter()
                    .map(|b| b.mcu.clone())
                    .collect::<HashSet<_>>()
                    .into_iter()
                    .collect::<Vec<_>>();

                if mcus.len() > 1 {
                    bail!(
                        "Configured platform '{}' and frameworks [{}] match multiple MCUs in PIO: [{}]",
                        params.platform.as_ref().unwrap(),
                        params.frameworks.join(", "),
                        mcus.join(", "));
                } else {
                    info!(
                        "Configuring MCU '{}' which supports configured platform '{}' and configured frameworks [{}]",
                        mcus[0],
                        params.platform.as_ref().unwrap(),
                        params.frameworks.join(", "));

                        params.mcu = Some(mcus[0].clone());
                }
            }

            info!(
                "Configuring board '{}' which supports configured platform '{}', MCU '{}' and configured frameworks [{}]",
                boards[0].id,
                params.platform.as_ref().unwrap(),
                params.mcu.as_ref().unwrap(),
                params.frameworks.join(", "));

                params.board = Some(boards[0].id.clone());
        }

        params.try_into()
    }

    fn get_default_platform_mcu_frameworks(&self) -> Result<TargetConf> {
        if let Some(ref target) = self.target {
            Ok(match target.as_str() {
                "esp32-xtensa-none" => TargetConf {
                    platform: "espressif32",
                    mcu: "esp32",
                    frameworks: vec!["espidf", "arduino", "simba", "pumbaa"],
                },
                "esp8266-xtensa-none" => TargetConf {
                    platform: "espressif8266",
                    mcu: "esp8266",
                    frameworks: vec!["esp8266-rtos-sdk", "esp8266-nonos-sdk", "ardino", "simba"],
                },
                _ => bail!("Cannot derive default PIO platform, MCU and frameworks for target '{}'", target),
            })
        } else {
            bail!("No target")
        }
    }
}