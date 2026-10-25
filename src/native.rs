use crate::{legacy, model::World};
use flate2::{Compression, read::GzDecoder, write::GzEncoder};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{Read, Write},
    path::Path,
};

const MAGIC: &[u8] = b"CELLLAB2\0\x01";

#[derive(Serialize, Deserialize)]
struct Save {
    format_version: u32,
    world: World,
}

#[derive(Serialize)]
struct SaveRef<'a> {
    format_version: u32,
    world: &'a World,
}

pub fn load(path: impl AsRef<Path>) -> Result<World, String> {
    let bytes = fs::read(path).map_err(|e| e.to_string())?;
    if bytes.starts_with(MAGIC) {
        let mut json = Vec::new();
        GzDecoder::new(&bytes[MAGIC.len()..])
            .read_to_end(&mut json)
            .map_err(|e| e.to_string())?;
        let save: Save = serde_json::from_slice(&json).map_err(|e| e.to_string())?;
        if save.format_version != 1 {
            return Err(format!(
                "Unsupported CellLab2 save version {}",
                save.format_version
            ));
        }
        Ok(save.world)
    } else {
        legacy::import(&bytes)
    }
}

pub fn save(path: impl AsRef<Path>, world: &World) -> Result<(), String> {
    let path = path.as_ref();
    let bytes = if path
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("substrate"))
    {
        legacy::export(world)?
    } else {
        let json = serde_json::to_vec(&SaveRef {
            format_version: 1,
            world,
        })
        .map_err(|e| e.to_string())?;
        // serde_json encodes NaN and infinity as null, which cannot be read
        // back into numeric fields. Validate only when null occurs.
        if json.windows(4).any(|chunk| chunk == b"null") {
            serde_json::from_slice::<Save>(&json)
                .map_err(|error| format!("Save contains invalid numeric state: {error}"))?;
        }
        let mut compressor = GzEncoder::new(Vec::new(), Compression::fast());
        compressor.write_all(&json).map_err(|e| e.to_string())?;
        let compressed = compressor.finish().map_err(|e| e.to_string())?;
        let mut bytes = Vec::with_capacity(MAGIC.len() + compressed.len());
        bytes.extend_from_slice(MAGIC);
        bytes.extend_from_slice(&compressed);
        bytes
    };
    let extension = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("celllab2");
    let temporary = path.with_extension(format!("{extension}.tmp"));
    fs::write(&temporary, bytes).map_err(|e| e.to_string())?;
    if path.exists() {
        let backup = path.with_extension(format!("{extension}.bak"));
        fs::copy(path, &backup).map_err(|e| e.to_string())?;
        if let Err(error) = fs::rename(&temporary, path) {
            let _ = fs::remove_file(&temporary);
            return Err(error.to_string());
        }
    } else {
        fs::rename(&temporary, path).map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_saved_state(name: &str, expected: &World, actual: &World) {
        assert_eq!(actual.time, expected.time, "{name}: simulation time");
        assert_eq!(
            actual.light_angle, expected.light_angle,
            "{name}: light angle"
        );
        assert_eq!(
            actual.environment, expected.environment,
            "{name}: environment"
        );
        assert_eq!(actual.food, expected.food, "{name}: food state");
        assert_eq!(
            actual.cells.len(),
            expected.cells.len(),
            "{name}: cell count"
        );
        for (index, (before, after)) in expected.cells.iter().zip(&actual.cells).enumerate() {
            let mut before = before.clone();
            let mut after = after.clone();
            before.runtime_id = 0;
            after.runtime_id = 0;
            if before != after {
                let first_link = before
                    .links
                    .iter()
                    .zip(&after.links)
                    .position(|(left, right)| left != right);
                let first_gene = before
                    .genes
                    .iter()
                    .zip(&after.genes)
                    .position(|(left, right)| left != right);
                panic!(
                    "{name}: organism {index}; links {} -> {}, first differing link {:?}; genes {} -> {}, first differing gene {:?}",
                    before.links.len(),
                    after.links.len(),
                    first_link.map(|n| (n, &before.links[n], &after.links[n])),
                    before.genes.len(),
                    after.genes.len(),
                    first_gene.map(|n| (n, &before.genes[n], &after.genes[n]))
                );
            }
        }
    }

    #[test]
    #[ignore = "full 65-plate save audit; run explicitly"]
    fn every_bundled_plate_roundtrips_both_formats() {
        for (name, bytes) in crate::bundled::PLATES {
            let mut world = legacy::import(bytes)
                .unwrap_or_else(|error| panic!("{name}: import failed: {error}"));
            world.environment.max_cells = world.environment.max_cells.max(world.cells.len());
            world.environment.max_food = world.environment.max_food.max(world.food.len());
            let native_json = serde_json::to_vec(&SaveRef {
                format_version: 1,
                world: &world,
            })
            .unwrap_or_else(|error| panic!("{name}: native encode failed: {error}"));
            let native: Save = serde_json::from_slice(&native_json)
                .unwrap_or_else(|error| panic!("{name}: native decode failed: {error}"));
            assert_saved_state(&format!("{name} native"), &world, &native.world);
            let exported = legacy::export(&world)
                .unwrap_or_else(|error| panic!("{name}: export failed: {error}"));
            let restored = legacy::import(&exported)
                .unwrap_or_else(|error| panic!("{name}: reimport failed: {error}"));
            assert_saved_state(&format!("{name} original"), &world, &restored);
        }
    }

    #[test]
    fn bundled_hunt_plate_roundtrips_after_invalid_rotation_repair() {
        let bytes = crate::bundled::PLATES
            .iter()
            .find(|(name, _)| *name == "hunt")
            .unwrap()
            .1;
        let world = legacy::import(bytes).unwrap();
        assert!(
            world
                .cells
                .iter()
                .all(|cell| cell.values.iter().all(|value| value.is_finite()))
        );
        let path = std::env::temp_dir().join(format!(
            "celllab2-hunt-test-{}.celllab2",
            std::process::id()
        ));
        save(&path, &world).unwrap();
        let loaded = load(&path).unwrap();
        assert_eq!(loaded.cells.len(), world.cells.len());
        assert_eq!(loaded.food.len(), world.food.len());
        let _ = fs::remove_file(path);
    }

    #[test]
    fn native_save_rejects_nonfinite_state_before_writing() {
        let mut world = legacy::import(crate::bundled::PLATES[0].1).unwrap();
        world.cells[0].values[7] = f64::NAN;
        let path = std::env::temp_dir().join(format!(
            "celllab2-invalid-test-{}.celllab2",
            std::process::id()
        ));
        assert!(save(&path, &world).is_err());
        assert!(!path.exists());
    }

    #[test]
    fn imported_save_roundtrip_keeps_edits_and_organisms() {
        let mut world = legacy::import(crate::bundled::PLATES[0].1).unwrap();
        let cells = world.cells.len();
        let food = world.food.len();
        world.set_radius(1.4);
        world.set_max_cells(2_000);
        world.set_max_food(6_000);
        let path =
            std::env::temp_dir().join(format!("celllab2-test-{}.celllab2", std::process::id()));
        save(&path, &world).unwrap();
        let loaded = load(&path).unwrap();
        assert_eq!(loaded.cells.len(), cells);
        assert_eq!(loaded.food.len(), food);
        assert_eq!(loaded.environment.radius, 1.4);
        assert_eq!(loaded.environment.max_cells, 2_000);
        assert_eq!(loaded.environment.max_food, 6_000);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn original_format_export_reimports_with_edits() {
        let mut world = legacy::import(crate::bundled::PLATES[0].1).unwrap();
        world.set_radius(1.6);
        world.set_max_cells(2_000);
        world.set_max_food(6_000);
        let path =
            std::env::temp_dir().join(format!("celllab2-test-{}.substrate", std::process::id()));
        save(&path, &world).unwrap();
        let loaded = load(&path).unwrap();
        assert_eq!(loaded.source_version, Some(95));
        assert_eq!(loaded.cells.len(), world.cells.len());
        assert_eq!(loaded.food.len(), world.food.len());
        assert_eq!(loaded.environment.radius, 1.6);
        assert_eq!(loaded.environment.max_cells, 2_000);
        assert_eq!(loaded.environment.max_food, 6_000);
        assert_eq!(
            loaded.cells[0].genes[0].cell_type,
            world.cells[0].genes[0].cell_type
        );
        let _ = fs::remove_file(path);
    }

    #[test]
    fn lowered_limits_preserve_existing_contents_in_both_save_formats() {
        let mut world = legacy::import(crate::bundled::PLATES[0].1).unwrap();
        assert!(!world.cells.is_empty());
        assert!(!world.food.is_empty());
        let cell_count = world.cells.len();
        let food_count = world.food.len();
        world.set_radius(1.5);
        world.set_max_cells(0);
        world.set_max_food(0);

        for extension in ["celllab2", "substrate"] {
            let path = std::env::temp_dir().join(format!(
                "celllab2-lowered-limits-{}.{}",
                std::process::id(),
                extension
            ));
            save(&path, &world).unwrap();
            let loaded = load(&path).unwrap();
            assert_eq!(loaded.cells.len(), cell_count);
            assert_eq!(loaded.food.len(), food_count);
            assert_eq!(loaded.environment.radius, 1.5);
            assert_eq!(loaded.environment.max_cells, 0);
            assert_eq!(loaded.environment.max_food, 0);
            fs::remove_file(path).unwrap();
        }
    }

    #[test]
    fn replacing_an_existing_save_keeps_a_readable_backup_in_both_formats() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        for extension in ["celllab2", "substrate"] {
            let path = std::env::temp_dir().join(format!(
                "celllab2-replace-test-{}-{stamp}.{extension}",
                std::process::id()
            ));
            let backup = path.with_extension(format!("{extension}.bak"));
            let mut world = legacy::import(crate::bundled::PLATES[0].1).unwrap();
            world.set_radius(1.2);
            save(&path, &world).unwrap();
            let first_bytes = fs::read(&path).unwrap();
            world.set_radius(1.8);
            save(&path, &world).unwrap();
            assert_eq!(fs::read(&backup).unwrap(), first_bytes);
            assert_eq!(load(&path).unwrap().environment.radius, 1.8);
            assert_eq!(load(&backup).unwrap().environment.radius, 1.2);
            fs::remove_file(path).unwrap();
            fs::remove_file(backup).unwrap();
        }
    }
}
