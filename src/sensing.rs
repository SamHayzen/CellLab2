use crate::{
    model::{Cell, World},
    sense_kernel::{self, DIRECTIONAL, SCALAR},
    sim::{light_at, light_gradient},
};
use rayon::prelude::*;

struct SenseGrid {
    heads: Vec<usize>,
    next: Vec<usize>,
    side: usize,
    bin: f64,
    radius: f64,
}

impl SenseGrid {
    fn build(len: usize, radius: f64, mut position: impl FnMut(usize) -> (f64, f64)) -> Self {
        let radius = radius.max(0.05);
        let bin = (2.0 * radius / (len.max(1) as f64).sqrt()).max(0.09);
        Self::build_with_bin(len, radius, bin, &mut position)
    }

    fn build_with_bin(
        len: usize,
        radius: f64,
        bin: f64,
        mut position: impl FnMut(usize) -> (f64, f64),
    ) -> Self {
        let side = ((2.0 * radius / bin).ceil() as usize) + 3;
        let mut grid = Self {
            heads: vec![usize::MAX; side * side],
            next: vec![usize::MAX; len],
            side,
            bin,
            radius,
        };
        for index in 0..len {
            let (x, y) = position(index);
            let flat = grid.coordinate(y) * side + grid.coordinate(x);
            grid.next[index] = grid.heads[flat];
            grid.heads[flat] = index;
        }
        grid
    }

    fn coordinate(&self, value: f64) -> usize {
        (((value + self.radius) / self.bin).floor() as isize).clamp(0, self.side as isize - 1)
            as usize
    }

    fn visit(&self, x: f64, y: f64, range: f64, mut inspect: impl FnMut(usize)) {
        for by in self.coordinate(y - range)..=self.coordinate(y + range) {
            for bx in self.coordinate(x - range)..=self.coordinate(x + range) {
                let mut index = self.heads[by * self.side + bx];
                while index != usize::MAX {
                    inspect(index);
                    index = self.next[index];
                }
            }
        }
    }

    fn visit_neighbor_bins(&self, x: f64, y: f64, mut inspect: impl FnMut(usize)) {
        let bx = self.coordinate(x);
        let by = self.coordinate(y);
        for row in by.saturating_sub(1)..=(by + 1).min(self.side - 1) {
            for column in bx.saturating_sub(1)..=(bx + 1).min(self.side - 1) {
                let mut index = self.heads[row * self.side + column];
                while index != usize::MAX {
                    inspect(index);
                    index = self.next[index];
                }
            }
        }
    }
}

pub fn update_signals(world: &mut World, dt: f64) {
    if world.cells.is_empty() || dt <= 0.0 {
        return;
    }
    let kinds: Vec<i32> = world.cells.iter().map(Cell::cell_type).collect();
    let mut has_sensors = false;
    let mut sense_cells = false;
    let mut sense_food = false;
    let mut sense_scent = false;
    for (cell, kind) in world.cells.iter().zip(&kinds) {
        if !matches!(kind, 10 | 11) {
            continue;
        }
        has_sensors = true;
        if let Some(gene) = cell.active_gene() {
            let mode = gene.output_modes[3];
            sense_cells |= mode == 0;
            sense_food |= matches!(mode, 1 | 2);
            sense_scent |= matches!(mode, 1..=3 | 5);
        }
    }
    let scent_sources: Vec<usize> = if sense_scent {
        world
            .cells
            .iter()
            .enumerate()
            .filter_map(|(index, cell)| {
                (kinds[index] == 14
                    && cell.tuvp[3] >= 0.2
                    && cell
                        .active_gene()
                        .is_some_and(|gene| matches!(gene.output_modes[4], 0 | 4 | 5 | 6)))
                .then_some(index)
            })
            .collect()
    } else {
        Vec::new()
    };
    let cell_grid = sense_cells.then(|| {
        SenseGrid::build(world.cells.len(), world.environment.radius, |i| {
            (world.cells[i].x, world.cells[i].y)
        })
    });
    let food_grid = sense_food.then(|| {
        SenseGrid::build(world.food.len(), world.environment.radius, |i| {
            (world.food[i].x as f64, world.food[i].y as f64)
        })
    });
    let scent_grid = (!scent_sources.is_empty()).then(|| {
        let radius = world.environment.radius.max(0.05);
        let bins = ((2.0 * radius / 0.36) as usize).clamp(1, 207);
        SenseGrid::build_with_bin(
            scent_sources.len(),
            radius,
            2.0 * radius / bins as f64,
            |i| {
                let cell = &world.cells[scent_sources[i]];
                (cell.x, cell.y)
            },
        )
    });
    let sensor_values: Vec<f32> = if has_sensors {
        world
            .cells
            .par_iter()
            .enumerate()
            .map(|(index, cell)| {
                if matches!(kinds[index], 10 | 11) {
                    sensed_value(
                        index,
                        cell,
                        world,
                        cell_grid.as_ref(),
                        food_grid.as_ref(),
                        scent_grid.as_ref(),
                        &scent_sources,
                    ) as f32
                } else {
                    0.0
                }
            })
            .collect()
    } else {
        Vec::new()
    };

    let quarter = (dt / 4.0) as f32;
    // A cell needs synchronized substeps only when a live link can carry
    // neural state to or from another cell. Isolated cells can perform the
    // same four substeps locally, avoiding four whole-population barriers.
    let mut linked = vec![false; world.cells.len()];
    for (index, cell) in world.cells.iter().enumerate() {
        if kinds[index] == 4 {
            continue;
        }
        for link in cell.links.iter().take(cell.c.max(0) as usize) {
            if link.target >= 0 {
                let other = link.target as usize;
                if kinds.get(other).is_some_and(|kind| *kind != 4) {
                    linked[index] = true;
                    linked[other] = true;
                }
            }
        }
    }
    let linked_indices: Vec<usize> = linked
        .iter()
        .enumerate()
        .filter_map(|(index, is_linked)| is_linked.then_some(index))
        .collect();
    world
        .cells
        .par_iter_mut()
        .enumerate()
        .for_each(|(index, cell)| {
            if linked[index] {
                return;
            }
            if !matches!(kinds[index], 10 | 11 | 13)
                && cell.u.iter().all(|value| value.to_bits() == 0)
            {
                cell.w = [0.0; 4];
                return;
            }
            let sensor = if has_sensors {
                sensor_values[index]
            } else {
                0.0
            };
            for substep in 0..4 {
                let (next, produced) =
                    signal_substep(cell, kinds[index], sensor, None, &kinds, quarter);
                cell.u = next;
                if substep == 3 {
                    cell.w = produced;
                }
            }
        });
    if linked_indices.is_empty() {
        return;
    }
    let mut updates = vec![([0.0; 4], [0.0; 4]); linked_indices.len()];
    for substep in 0..4 {
        updates
            .par_iter_mut()
            .enumerate()
            .for_each(|(slot, update)| {
                let index = linked_indices[slot];
                let sensor = if has_sensors {
                    sensor_values[index]
                } else {
                    0.0
                };
                *update = signal_substep(
                    &world.cells[index],
                    kinds[index],
                    sensor,
                    Some(&world.cells),
                    &kinds,
                    quarter,
                );
            });
        for (&index, &(next, produced)) in linked_indices.iter().zip(&updates) {
            let cell = &mut world.cells[index];
            cell.u = next;
            if substep == 3 {
                cell.w = produced;
            }
        }
    }
}

fn signal_substep(
    cell: &Cell,
    kind: i32,
    sensor: f32,
    cells: Option<&[Cell]>,
    kinds: &[i32],
    quarter: f32,
) -> ([f32; 4], [f32; 4]) {
    let mut produced = [0.0; 4];
    if let Some(gene) = cell.active_gene() {
        match kind {
            13 => {
                for output in 0..4 {
                    let channel = gene.output_modes[output + 5].clamp(0, 3) as usize;
                    let value = cell.signal(output + 7).clamp(-20.0, 20.0);
                    if value.is_finite() {
                        produced[channel] += value;
                    }
                }
            }
            10 | 11 => {
                let channel = gene.output_modes[2].clamp(0, 3) as usize;
                let value = (gene.output_values[0] * sensor).clamp(-20.0, 20.0);
                if value.is_finite() {
                    produced[channel] += value;
                }
            }
            _ => {}
        }
    }
    let mut next = [0.0; 4];
    for channel in 0..4 {
        let mut derivative = produced[channel] - 5.0 * cell.u[channel];
        if kind != 4 {
            if let Some(cells) = cells {
                for link in cell.links.iter().take(cell.c.max(0) as usize) {
                    if link.target >= 0 {
                        let other = link.target as usize;
                        if kinds.get(other).is_some_and(|kind| *kind != 4) {
                            derivative += 5.0 * (cells[other].u[channel] - cell.u[channel]);
                        }
                    }
                }
            }
        }
        let value = cell.u[channel] + derivative * quarter;
        next[channel] = if value.is_finite() {
            value.clamp(-1.0, 1.0)
        } else {
            0.0
        };
    }
    (next, produced)
}

fn sensed_value(
    index: usize,
    cell: &Cell,
    world: &World,
    cell_grid: Option<&SenseGrid>,
    food_grid: Option<&SenseGrid>,
    scent_grid: Option<&SenseGrid>,
    scent_sources: &[usize],
) -> f64 {
    let Some(gene) = cell.active_gene() else {
        return 0.0;
    };
    let directional = gene.cell_type == 10;
    let (sin, cos) = if directional {
        cell.values[0].sin_cos()
    } else {
        (0.0, 0.0)
    };
    let radius = world.environment.radius;
    let mut sensed = match gene.output_modes[3] {
        3 => {
            if directional {
                let (dx, dy) = light_gradient(
                    cell.x,
                    cell.y,
                    radius,
                    world.light_angle,
                    world.environment.d,
                    world.environment.f,
                );
                -(dx * sin - dy * cos) * 0.2
            } else {
                light_at(
                    cell.x,
                    cell.y,
                    radius,
                    world.light_angle,
                    world.environment.d,
                    world.environment.f,
                ) * 0.2
            }
        }
        4 => {
            if directional {
                (cell.values[2] * sin - cell.values[3] * cos) * 3.0
            } else {
                cell.values[2].hypot(cell.values[3]) * 3.0
            }
        }
        5 => {
            let distance = cell.x.hypot(cell.y);
            let wall = (1.0 - (radius - distance) / 0.27).clamp(0.0, 1.0);
            if directional && distance > 0.0 {
                -wall * (cell.x * sin - cell.y * cos) / distance
            } else {
                wall
            }
        }
        _ => 0.0,
    };
    const RANGE: f64 = 0.27;
    match gene.output_modes[3] {
        0 => {
            if let Some(grid) = cell_grid {
                grid.visit(cell.x, cell.y, RANGE, |other_index| {
                    if index == other_index {
                        return;
                    }
                    let other = &world.cells[other_index];
                    let color_distance: f64 = (0..3)
                        .map(|channel| {
                            let difference = gene.output_values[channel + 1] as f64
                                - other.color[channel] as f64;
                            difference * difference
                        })
                        .sum();
                    if color_distance >= (gene.output_values[4] as f64).powi(2) {
                        return;
                    }
                    sensed += source_strength(
                        cell,
                        other.x,
                        other.y,
                        other.values[7] * 10.0,
                        directional,
                        sin,
                        cos,
                    );
                });
            }
        }
        1 | 2 => {
            if let Some(grid) = food_grid {
                grid.visit(cell.x, cell.y, RANGE, |food_index| {
                    let food = &world.food[food_index];
                    let matches = if gene.output_modes[3] == 1 {
                        food.state <= 0.0
                    } else {
                        food.state > 0.0
                    };
                    if matches {
                        sensed += source_strength(
                            cell,
                            food.x as f64,
                            food.y as f64,
                            food.energy as f64 * 10.0,
                            directional,
                            sin,
                            cos,
                        );
                    }
                });
            }
        }
        _ => {}
    }
    // CellWorld.g()/d(): Secrocytes also occupy a smell-grid slot. Their
    // virtual source is a quarter of the physical distance from the sensor.
    let scent_mode = match gene.output_modes[3] {
        1 => Some(0),
        2 => Some(4),
        3 => Some(5),
        5 => Some(6),
        _ => None,
    };
    if let (Some(grid), Some(mode)) = (scent_grid, scent_mode) {
        grid.visit_neighbor_bins(cell.x, cell.y, |slot| {
            let source = &world.cells[scent_sources[slot]];
            if source
                .active_gene()
                .is_some_and(|gene| gene.output_modes[4] == mode)
            {
                sensed += source_strength(
                    cell,
                    cell.x + (source.x - cell.x) * 0.25,
                    cell.y + (source.y - cell.y) * 0.25,
                    source.values[7] * 10.0,
                    directional,
                    sin,
                    cos,
                );
            }
        });
    }
    if directional && cell.y_flag {
        -sensed
    } else {
        sensed
    }
}

fn source_strength(
    cell: &Cell,
    x: f64,
    y: f64,
    mass: f64,
    directional: bool,
    sin: f64,
    cos: f64,
) -> f64 {
    let dx = cell.x - x;
    let dy = cell.y - y;
    let distance = dx.hypot(dy);
    if distance <= 0.0 || distance > 0.27 {
        return 0.0;
    }
    if directional {
        mass * sense_kernel::sample(&DIRECTIONAL, distance) * (dx * sin - dy * cos) / distance
    } else {
        mass * sense_kernel::sample(&SCALAR, distance)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{bundled, legacy, model::GeneInput};

    #[test]
    fn neurocyte_produces_all_four_signal_channels() {
        let mut world = legacy::import(bundled::PLATES[0].1).unwrap();
        world.cells.truncate(1);
        world.cells[0].mode = 0;
        world.cells[0].genes[0].cell_type = 13;
        for output in 0..4 {
            world.cells[0].genes[0].output_modes[output + 5] = output as i32;
            world.cells[0].genes[0].inputs[output + 7] = GeneInput {
                source: 0,
                mode: 0,
                a: 0.0,
                b: 1.0,
                c: 0.0,
            };
        }
        world.cells[0].u = [0.0; 4];
        update_signals(&mut world, 0.02);
        assert!(world.cells[0].u.iter().all(|value| *value > 0.3));
        assert!(world.cells[0].w.iter().all(|value| *value == 20.0));
    }

    #[test]
    fn directional_sensor_detects_nearby_food() {
        let mut world = legacy::import(bundled::PLATES[0].1).unwrap();
        world.cells.truncate(1);
        world.food.truncate(1);
        let cell = &mut world.cells[0];
        cell.mode = 0;
        cell.x = 0.0;
        cell.y = 0.0;
        cell.values[0] = 0.0;
        cell.y_flag = false;
        cell.u = [0.0; 4];
        cell.genes[0].cell_type = 10;
        cell.genes[0].output_modes[2] = 1;
        cell.genes[0].output_modes[3] = 1;
        cell.genes[0].output_values[0] = 1.0;
        world.food[0].x = 0.0;
        world.food[0].y = 0.04;
        world.food[0].energy = 0.2;
        world.food[0].state = 0.0;
        update_signals(&mut world, 0.02);
        assert!(world.cells[0].w[1] > 0.0);
        assert!(world.cells[0].u[1] > 0.0);
        assert_eq!(world.cells[0].w[0], 0.0);
    }

    #[test]
    fn sensors_detect_secrocyte_scents_on_the_apk_channels() {
        let mut world = legacy::import(bundled::PLATES[0].1).unwrap();
        world.cells.truncate(2);
        world.food.clear();
        world.environment.d = 0.0;
        world.cells[0].mode = 0;
        world.cells[0].genes[0].cell_type = 11;
        world.cells[0].genes[0].output_modes[2] = 0;
        world.cells[0].genes[0].output_values[0] = 1.0;
        world.cells[0].x = 0.0;
        world.cells[0].y = 0.0;
        world.cells[1].mode = 0;
        world.cells[1].genes[0].cell_type = 14;
        world.cells[1].x = 0.2;
        world.cells[1].y = 0.0;
        world.cells[1].values[7] = 0.2;
        world.cells[1].tuvp[3] = 1.0;

        world.environment.radius = 2.5;
        for (sensor_mode, secretion_mode) in [(1, 0), (2, 4), (3, 5), (5, 6)] {
            world.cells[0].genes[0].output_modes[3] = sensor_mode;
            world.cells[1].genes[0].output_modes[4] = secretion_mode;
            update_signals(&mut world, 0.02);
            assert!(world.cells[0].w[0] > 0.0, "sensor mode {sensor_mode}");
        }
        world.cells[1].tuvp[3] = 0.1;
        update_signals(&mut world, 0.02);
        assert_eq!(world.cells[0].w[0], 0.0);
    }

    #[test]
    fn linked_cells_exchange_signal_but_lipocytes_block_it() {
        let mut world = legacy::import(bundled::PLATES[0].1).unwrap();
        world.cells.truncate(2);
        world.cells[0].mode = 0;
        world.cells[1].mode = 0;
        world.cells[0].genes[0].cell_type = 0;
        world.cells[1].genes[0].cell_type = 1;
        world.cells[0].u = [0.0; 4];
        world.cells[1].u = [1.0, 0.0, 0.0, 0.0];
        world.cells[0].links.clear();
        world.cells[0].links.push(crate::model::Link {
            target: 1,
            c: 0.0,
            d: 0.0,
            b: false,
            e: 0.0,
            f: 0.0,
            k: 0.0,
            l: 0.0,
        });
        world.cells[0].c = 1;
        update_signals(&mut world, 0.02);
        assert!(world.cells[0].u[0] > 0.08);

        world.cells[0].genes[0].cell_type = 4;
        world.cells[0].u = [0.0; 4];
        world.cells[1].u = [1.0, 0.0, 0.0, 0.0];
        update_signals(&mut world, 0.02);
        assert_eq!(world.cells[0].u[0], 0.0);
    }

    #[test]
    fn isolated_and_linked_updates_match_four_global_substeps() {
        let mut world = legacy::import(bundled::PLATES[0].1).unwrap();
        let template = world.cells[0].clone();
        world.cells = (0..8).map(|_| template.clone()).collect();
        for (index, cell) in world.cells.iter_mut().enumerate() {
            cell.mode = 0;
            cell.genes[0].cell_type = 0;
            cell.links.clear();
            cell.c = 0;
            cell.u = [index as f32 * 0.11, -0.2, 0.3, -0.4];
            cell.w = [1.0; 4];
        }
        for index in [0, 3] {
            let cell = &mut world.cells[index];
            cell.genes[0].cell_type = 13;
            cell.genes[0].output_modes[5] = 0;
            cell.genes[0].inputs[7] = GeneInput {
                source: 0,
                mode: 1,
                a: 1.0,
                b: 0.0,
                c: 0.0,
            };
        }
        world.cells[2].genes[0].cell_type = 4;
        world.cells[6].u = [0.0; 4];
        world.cells[7].u = [-0.0; 4];
        for (source, target) in [(0, 1), (1, 0), (2, 1), (4, 5)] {
            world.cells[source].links.push(crate::model::Link {
                target,
                c: 0.0,
                d: 0.0,
                b: false,
                e: 0.0,
                f: 0.0,
                k: 0.0,
                l: 0.0,
            });
            world.cells[source].c += 1;
        }
        let mut reference = world.clone();
        for _ in 0..3 {
            let kinds: Vec<i32> = reference.cells.iter().map(Cell::cell_type).collect();
            let quarter = (0.02_f64 / 4.0) as f32;
            for substep in 0..4 {
                let updates: Vec<_> = reference
                    .cells
                    .iter()
                    .enumerate()
                    .map(|(index, cell)| {
                        signal_substep(
                            cell,
                            kinds[index],
                            0.0,
                            Some(&reference.cells),
                            &kinds,
                            quarter,
                        )
                    })
                    .collect();
                for (cell, (next, produced)) in reference.cells.iter_mut().zip(updates) {
                    cell.u = next;
                    if substep == 3 {
                        cell.w = produced;
                    }
                }
            }
            update_signals(&mut world, 0.02);
            for (actual, expected) in world.cells.iter().zip(&reference.cells) {
                assert_eq!(actual.u.map(f32::to_bits), expected.u.map(f32::to_bits));
                assert_eq!(actual.w.map(f32::to_bits), expected.w.map(f32::to_bits));
            }
        }
    }
}
