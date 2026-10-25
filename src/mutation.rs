//! Genome mutation from CellWorld.g(): one opportunity every 15 simulation ticks.

use crate::model::{Cell, Environment};
use rand::Rng;

const MODE_COUNTS: [i32; 11] = [40, 40, 4, 6, 15, 4, 4, 4, 4, 20, 6];
const OUTPUT_LOW: [f32; 7] = [-60.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.5];
const OUTPUT_HIGH: [f32; 7] = [60.0, 1.0, 1.0, 1.0, 1.732_050_8, 0.01, 2.0];

pub fn tick(cell: &mut Cell, environment: &Environment, dt: f64) {
    if !advance_clock(cell) || environment.c <= 0.0 || dt <= 0.0 || cell.genes.is_empty() {
        return;
    }
    let mut rng = rand::rng();
    mutate_due(cell, environment, dt, &mut rng);
}

fn advance_clock(cell: &mut Cell) -> bool {
    cell.mutation_ticks += 1;
    if cell.mutation_ticks < 15 {
        return false;
    }
    cell.mutation_ticks = 0;
    true
}

fn mutate_due(cell: &mut Cell, environment: &Environment, dt: f64, rng: &mut impl Rng) {
    if rng.random::<f64>() >= environment.c * dt * 200.0 {
        return;
    }
    for _ in 0..3 {
        if rng.random_range(0..5) != 0 {
            cell.g = cell.g.saturating_add(1);
            let gene_count = cell.genes.len();
            let index = rng.random_range(0..gene_count);
            let mutation = rng.random_range(0..95);
            mutate_gene(
                &mut cell.genes[index],
                mutation,
                environment,
                gene_count,
                rng,
            );
        } else if !environment.u {
            cell.g = cell.g.saturating_add(1);
            rewire_genes(cell, rng);
        }
    }
}

fn mutate_gene(
    gene: &mut crate::model::Gene,
    mutation: usize,
    environment: &Environment,
    gene_count: usize,
    rng: &mut impl Rng,
) {
    match mutation {
        0 => {
            for color in &mut gene.color {
                *color = (*color as f64 + gaussian(rng) * 0.15).clamp(0.0, 1.0) as f32;
            }
        }
        1..=3 => {
            let value = gene.floats[mutation + 1] as f64 + gaussian(rng);
            gene.floats[mutation + 1] = value.rem_euclid(std::f64::consts::TAU) as f32;
        }
        4 => {
            gene.floats[5] =
                ((gene.floats[5] as f64 * (gaussian(rng) * 0.5).exp()) as f32).clamp(0.1, 10.0);
        }
        5 => {
            gene.floats[0] = ((gene.floats[0] as f64 * (gaussian(rng) * 0.5).exp()) as f32)
                .clamp(0.0648, 0.3672);
        }
        6 => {
            gene.floats[1] = (gene.floats[1] as f64 + gaussian(rng) * 0.2).clamp(0.1, 0.9) as f32;
        }
        7 if !environment.u => gene.i = rng.random_range(0..gene_count) as i32,
        8 if !environment.u => gene.j = rng.random_range(0..gene_count) as i32,
        9..=11 if !environment.u => gene.flags[mutation - 9] ^= true,
        12 if !environment.u => {
            let allowed: Vec<usize> = environment
                .enabled_types
                .iter()
                .take(18)
                .enumerate()
                .filter_map(|(index, enabled)| (*enabled).then_some(index))
                .collect();
            if !allowed.is_empty() {
                gene.cell_type = allowed[rng.random_range(0..allowed.len())] as i32;
            }
        }
        14 => gene.flags[3] ^= true,
        18 => gene.flags[5] ^= true,
        19 => gene.flags[6] ^= true,
        20 => gene.floats[6] = bounded_perturb(0.0, 35.0, gene.floats[6], rng),
        22..=32 if !environment.u && mutation != 32 => {
            let index = mutation - 22;
            gene.output_modes[index] = rng.random_range(0..MODE_COUNTS[index]);
        }
        33..=43 if !environment.u => {
            gene.inputs[mutation - 33].source = rng.random_range(0..8);
        }
        44..=54 if !environment.u => {
            gene.inputs[mutation - 44].mode = rng.random_range(0..3);
        }
        55..=65 => {
            let input = &mut gene.inputs[mutation - 55];
            input.a = bounded_perturb(-8.0, 8.0, input.a, rng);
        }
        66..=76 => {
            let input = &mut gene.inputs[mutation - 66];
            input.b = bounded_perturb(-8.0, 8.0, input.b, rng);
        }
        77..=87 => {
            let input = &mut gene.inputs[mutation - 77];
            input.c = bounded_perturb(-8.0, 8.0, input.c, rng);
        }
        88..=94 => {
            let index = mutation - 88;
            gene.output_values[index] = bounded_perturb(
                OUTPUT_LOW[index],
                OUTPUT_HIGH[index],
                gene.output_values[index],
                rng,
            );
        }
        _ => {}
    }
}

/// Gene.a(low, high, old, rng): reset uniformly half the time, otherwise
/// make a small Gaussian change and clamp it to the editor's legal range.
fn bounded_perturb(low: f32, high: f32, old: f32, rng: &mut impl Rng) -> f32 {
    if rng.random::<bool>() {
        low + (high - low) * rng.random::<f32>()
    } else {
        (old as f64 + (high - low) as f64 * gaussian(rng) * 0.03).clamp(low as f64, high as f64)
            as f32
    }
}

fn gaussian(rng: &mut impl Rng) -> f64 {
    let radius = (-2.0 * (1.0 - rng.random::<f64>()).ln()).sqrt();
    let angle = std::f64::consts::TAU * rng.random::<f64>();
    radius * angle.cos()
}

fn rewire_genes(cell: &mut Cell, rng: &mut impl Rng) {
    let count = cell.genes.len();
    if count < 2 {
        return;
    }
    let parent = rng.random_range(0..count);
    let mut branch = rng.random_range(0..count - 1);
    if branch >= parent {
        branch += 1;
    }
    let left = rng.random::<bool>();
    match rng.random_range(0..3) {
        0 => {
            let old = if left {
                cell.genes[parent].i
            } else {
                cell.genes[parent].j
            };
            if let Some(copy) = usize::try_from(old)
                .ok()
                .and_then(|index| cell.genes.get(index))
                .cloned()
            {
                cell.genes[branch] = copy;
                if left {
                    cell.genes[parent].i = branch as i32;
                } else {
                    cell.genes[parent].j = branch as i32;
                }
            }
        }
        1 => {
            let old = if left {
                cell.genes[parent].i
            } else {
                cell.genes[parent].j
            };
            cell.genes[branch].flags[0] = true;
            cell.genes[branch].i = old;
            cell.genes[branch].j = old;
            if left {
                cell.genes[parent].i = branch as i32;
            } else {
                cell.genes[parent].j = branch as i32;
            }
        }
        _ if count >= 3 => {
            let choices: Vec<usize> = (0..count)
                .filter(|index| *index != parent && *index != branch)
                .collect();
            let third = choices[rng.random_range(0..choices.len())] as i32;
            let old = if left {
                let old = cell.genes[parent].i;
                cell.genes[parent].i = branch as i32;
                old
            } else {
                let old = cell.genes[parent].j;
                cell.genes[parent].j = branch as i32;
                old
            };
            if rng.random::<bool>() {
                cell.genes[branch].i = third;
                cell.genes[branch].j = old;
            } else {
                cell.genes[branch].i = old;
                cell.genes[branch].j = third;
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{bundled, legacy, sim};
    use rand::{SeedableRng, rngs::StdRng};

    #[test]
    fn plate_mutation_setting_changes_a_live_genome_and_survives_export() {
        let mut world = legacy::import(bundled::PLATES[0].1).unwrap();
        world.cells.truncate(1);
        world.environment.max_cells = 1;
        world.environment.c = 1.0;
        world.environment.u = false;
        let before = serde_json::to_vec(&world.cells[0].genes).unwrap();
        let original_count = world.cells[0].g;
        for _ in 0..15 {
            sim::step(&mut world, 0.02);
        }
        assert_eq!(world.cells.len(), 1);
        assert!(world.cells[0].g > original_count);
        let mut rng = StdRng::seed_from_u64(23);
        for _ in 0..10 {
            mutate_due(&mut world.cells[0], &world.environment, 0.02, &mut rng);
        }
        assert_ne!(serde_json::to_vec(&world.cells[0].genes).unwrap(), before);
        let exported = legacy::export(&world).unwrap();
        let reloaded = legacy::import(&exported).unwrap();
        assert_eq!(reloaded.cells[0].g, world.cells[0].g);
        assert_eq!(
            serde_json::to_vec(&reloaded.cells[0].genes).unwrap(),
            serde_json::to_vec(&world.cells[0].genes).unwrap()
        );
    }

    #[test]
    fn locked_plate_preserves_gene_topology_and_cell_types() {
        let mut world = legacy::import(bundled::PLATES[0].1).unwrap();
        let cell = &mut world.cells[0];
        world.environment.c = 1.0;
        world.environment.u = true;
        let original: Vec<_> = cell
            .genes
            .iter()
            .map(|gene| {
                (
                    gene.i,
                    gene.j,
                    gene.flags[..3].to_vec(),
                    gene.cell_type,
                    gene.output_modes,
                    gene.inputs.map(|input| (input.source, input.mode)),
                )
            })
            .collect();
        let mut rng = StdRng::seed_from_u64(12);
        for _ in 0..200 {
            mutate_due(cell, &world.environment, 0.02, &mut rng);
        }
        assert!(cell.g > 0);
        for (gene, expected) in cell.genes.iter().zip(original) {
            assert_eq!(gene.i, expected.0);
            assert_eq!(gene.j, expected.1);
            assert_eq!(gene.flags[..3], expected.2);
            assert_eq!(gene.cell_type, expected.3);
            assert_eq!(gene.output_modes, expected.4);
            assert_eq!(
                gene.inputs.map(|input| (input.source, input.mode)),
                expected.5
            );
        }
    }
}
