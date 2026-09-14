use std::collections::BTreeMap;

fn normalized_family_names<'a>(names: impl Iterator<Item = &'a str>) -> Vec<String> {
    let mut families = BTreeMap::new();
    for name in names {
        let name = name.trim();
        if !name.is_empty() && !name.starts_with('.') {
            families
                .entry(name.to_lowercase())
                .or_insert_with(|| name.to_string());
        }
    }
    families.into_values().collect()
}

fn discover_system_font_families() -> Vec<String> {
    let mut database = fontdb::Database::new();
    database.load_system_fonts();
    normalized_family_names(
        database
            .faces()
            .filter_map(|face| face.families.first().map(|(name, _)| name.as_str())),
    )
}

#[tauri::command]
pub async fn list_system_font_families() -> Result<Vec<String>, String> {
    tauri::async_runtime::spawn_blocking(discover_system_font_families)
        .await
        .map_err(|error| format!("Unable to discover system fonts: {error}"))
}

#[cfg(test)]
mod tests {
    use super::normalized_family_names;

    #[test]
    fn family_names_are_trimmed_deduplicated_and_sorted() {
        let names = ["Verdana", " Georgia ", "verdana", ".Hidden", ""];
        assert_eq!(
            normalized_family_names(names.into_iter()),
            vec!["Georgia".to_string(), "Verdana".to_string()]
        );
    }
}
