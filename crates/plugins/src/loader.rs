use std::path::PathBuf;

use moltis_skills::types::{SkillMetadata, SkillSource};

/// Plugin discovery and loading.
pub fn discover_plugins(_paths: &[PathBuf]) -> anyhow::Result<Vec<String>> {
    todo!("scan plugin directories, load manifests")
}

/// Scan plugin directories for bundled skills (each plugin may have a `skills/` subdirectory).
pub fn discover_plugin_skills(plugin_dirs: &[PathBuf]) -> Vec<SkillMetadata> {
    let mut skills = Vec::new();

    for plugin_dir in plugin_dirs {
        let skills_dir = plugin_dir.join("skills");
        if !skills_dir.is_dir() {
            continue;
        }
        let entries = match std::fs::read_dir(&skills_dir) {
            Ok(e) => e,
            Err(_) => continue,
        };

        for entry in entries.flatten() {
            let skill_dir = entry.path();
            if !skill_dir.is_dir() {
                continue;
            }
            let skill_md = skill_dir.join("SKILL.md");
            if !skill_md.is_file() {
                continue;
            }
            let content = match std::fs::read_to_string(&skill_md) {
                Ok(c) => c,
                Err(_) => continue,
            };
            if let Ok(mut meta) = moltis_skills::parse::parse_metadata(&content, &skill_dir) {
                meta.source = Some(SkillSource::Plugin);
                skills.push(meta);
            }
        }
    }

    skills
}
