use std::path::PathBuf;

use anyhow::{Context, Result};
use flate2::read::GzDecoder;
use tar::Archive;

mod api;
mod metadata;
mod hashing;
mod file_util;

/// A declared dependency on an ESP-IDF component.
pub struct IdfComponentDep {
    pub namespace: String,
    pub name: String,
    pub version_req: semver::VersionReq,
}

impl IdfComponentDep {
    pub fn new(namespace: String, name: String, version_req: semver::VersionReq) -> Self {
        Self {
            namespace,
            name,
            version_req,
        }
    }
}

#[derive(Debug)]
pub struct DepSolution {
    pub resolved_components: Vec<ResolvedIdfComponent>,
}

impl DepSolution {
    pub fn new(resolved_components: Vec<ResolvedIdfComponent>) -> Self {
        Self { resolved_components }
    }
}

/// A resolved dependency to an ESP-IDF component.
#[derive(Debug)]
pub struct ResolvedIdfComponent {
    pub namespace: String,
    pub name: String,
    pub version: semver::Version,
    pub component_hash: Option<String>,
    pub path: PathBuf,
}

impl ResolvedIdfComponent {
    pub fn new(namespace: String, name: String, version: semver::Version, component_hash: Option<String>, path: PathBuf) -> Self {
        Self { namespace, name, version, component_hash, path }
    }
}

pub struct IdfComponentManager {
    components_dir: PathBuf,
    pub components: Vec<IdfComponentDep>,
    api_client: api::Client,
}

impl IdfComponentManager {
    pub fn new(components_dir: PathBuf) -> Self {
        Self {
            components_dir,
            components: vec![],
            api_client: api::Client::new(),
        }
    }

    pub fn with_component(mut self, name: &str, version_spec: &str) -> Result<Self> {
        let version_req = semver::VersionReq::parse(version_spec)
            .context(format!("Error parsing version request for {}", name))?;

        // Parse namespace and name from component in format "namespace/name"
        match name.split('/').collect::<Vec<&str>>().as_slice() {
            [namespace, name] => {
                self.components.push(IdfComponentDep::new(
                    namespace.to_string(),
                    name.to_string(),
                    version_req,
                ));
            }
            _ => return Err(anyhow::anyhow!("Invalid component name {}", name)),
        }
        Ok(self)
    }

    pub fn install(&self) -> Result<DepSolution> {
        let mut components = vec![];
        for component in &self.components {
            let target_path = &self
                .components_dir
                .join(format!("{}__{}", component.namespace, component.name));

            println!(
                "Ensuring component '{}:{}' is installed...",
                component.name, component.version_req
            );
            let resolved_comp = self.resolve_component(component, target_path)?;
            components.push(resolved_comp);
        }
        let solution = DepSolution::new(components);
        Ok(solution)
    }

    fn resolve_component(
        &self,
        component: &IdfComponentDep,
        component_root: &PathBuf,
    ) -> Result<ResolvedIdfComponent> {
        // Check if installed component matches
        if metadata::installed_component_matches_version(&component.version_req, component_root)? {
            println!(
                "Component '{}' matching version spec '{}' is already installed.",
                component.name, component.version_req
            );
        } else {
            self.install_component(&component, component_root)?;
        }

        // Get hash from .component_hash
        let component_hash = hashing::read_hash_file(component_root)?;

        // Get metadata from `idf_component.yml`
        let metadata = metadata::read_component_metadata(component_root)?
            .expect("Component metadata file should exist after install");

        Ok(ResolvedIdfComponent::new(
            component.namespace.clone(),
            component.name.clone(),
            semver::Version::parse(&metadata.version).unwrap(),
            Some(component_hash),
            component_root.clone(),
        ))
    }

    fn install_component(&self, component: &&IdfComponentDep, target_path: &PathBuf) -> Result<()> {
        // Delete any old component that might be there
        if target_path.exists() {
            println!("Existing component '{}' in `{}` does not match version spec {}. Removing old version...",
                     component.name, target_path.display(), component.version_req);
            std::fs::remove_dir_all(target_path).context(format!(
                "Failed to remove old version of component '{}' at '{}'",
                component.name,
                target_path.display()
            ))?;
        }
        // Get metadata from the API
        let metadata = self
            .api_client
            .get_component(&component.namespace, &component.name)
            .context(format!(
                "Failed to get component '{}' from API",
                component.name
            ))?;

        // Construct a list of available versions in case we need to print it
        let available_versions = metadata
            .versions
            .iter()
            .filter(|v| v.yanked_at.is_none())
            .map(|v| v.version.clone())
            .collect::<Vec<_>>()
            .join(", ");

        // Find matching version
        let version = api::find_best_match(&metadata, &component.version_req)
            .context(format!("No matching version found for component '{}' with version spec '{}'. Available versions are: {}",
                             component.name, component.version_req, available_versions)
            )?;

        println!(
            "Downloading and unpacking component '{}:{}' from '{}' to '{}'...",
            component.name,
            version.version,
            version.url,
            target_path.display()
        );
        download_and_unpack(version.url.as_str(), target_path)?;
        let hash = hashing::hash_dir(target_path, vec![], true)?;
        hashing::write_hash_file(target_path, &hash)?;
        Ok(())
    }
}

fn download_and_unpack(tarball_url: &str, target_path: &PathBuf) -> Result<()> {
    let response = ureq::get(tarball_url).call()?;
    let mut tar = Archive::new(GzDecoder::new(response.into_reader()));
    tar.unpack(target_path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore]
    fn test_unpack() {
        let tmp_dir = tempdir::TempDir::new("managed_components")
            .unwrap()
            .into_path();

        let mgr = IdfComponentManager::new(tmp_dir)
            .with_component("espressif/mdns".into(), "1.1.0".into())
            .unwrap();

        let solution = mgr.install().unwrap();
        println!(
            "Final component path: {}",
            solution
                .resolved_components
                .iter()
                .map(|c| c.path.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
}
