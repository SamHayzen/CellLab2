//! Neurocyte oscillator preset from the APK's genome editor (`i.d`).

use crate::model::Gene;

const STEP: f32 = 0.005;

/// Simulate the two signal channels and any attached cells to estimate a cycle.
/// The APK measures eight half cycles and divides their elapsed time by four.
fn measured_period(connections: usize, gain: f32, coupling: f32) -> f32 {
    let mut first = 1.0f32;
    let mut second = 0.0f32;
    let mut linked_first = [0.0f32; 5];
    let mut linked_second = [0.0f32; 5];
    let mut crossings = 0;
    let mut sixth_crossing = 0.0;

    for iteration in 1..80_000 {
        let mut next_first = first
            + (-5.0 * first
                + (gain * first).clamp(-20.0, 20.0)
                + (coupling * second).clamp(-20.0, 20.0))
                * STEP;
        let mut next_second = second
            + (-5.0 * second
                + (gain * second).clamp(-20.0, 20.0)
                + (-coupling * first).clamp(-20.0, 20.0))
                * STEP;
        for index in 0..connections.min(5) {
            next_first -= 5.0 * (first - linked_first[index]) * STEP;
            next_second -= 5.0 * (second - linked_second[index]) * STEP;
            linked_first[index] +=
                (5.0 * (first - linked_first[index]) - 5.0 * linked_first[index]) * STEP;
            linked_second[index] +=
                (5.0 * (second - linked_second[index]) - 5.0 * linked_second[index]) * STEP;
        }
        if next_second * second < 0.0 {
            crossings += 1;
            if crossings == 6 {
                sixth_crossing = iteration as f32 * STEP;
            }
            if crossings == 14 {
                return (iteration as f32 * STEP - sixth_crossing) / 4.0;
            }
        }
        first = next_first.clamp(-1.0, 1.0);
        second = next_second.clamp(-1.0, 1.0);
    }
    (79_999.0 * STEP - sixth_crossing) / 4.0
}

pub fn apply_preset(
    gene: &mut Gene,
    connections: usize,
    first_channel: usize,
    second_channel: usize,
    period: f32,
) -> Result<(), &'static str> {
    if gene.cell_type != 13 {
        return Err("Oscillator preset requires a Neurocyte");
    }
    if connections > 5
        || first_channel >= 4
        || second_channel >= 4
        || first_channel == second_channel
    {
        return Err("Choose 0 to 5 connections and two different signal channels");
    }
    if !period.is_finite() || !(0.25..=9.25).contains(&period) {
        return Err("Period must be between 0.25 and 9.25 hours");
    }

    let gain = 7.5 + connections as f32 * 4.0;
    let mut lower = 0.0f32;
    let mut upper = 160.0f32;
    let mut coupling = (lower + upper) * 0.5;
    for _ in 0..25 {
        if measured_period(connections, gain, coupling) < period {
            upper = coupling;
        } else {
            lower = coupling;
        }
        coupling = (lower + upper) * 0.5;
    }

    for (index, (output, input, slope)) in [
        (first_channel, second_channel, coupling / 20.0),
        (second_channel, first_channel, -coupling / 20.0),
        (first_channel, first_channel, gain / 20.0),
        (second_channel, second_channel, gain / 20.0),
    ]
    .into_iter()
    .enumerate()
    {
        gene.output_modes[index + 5] = output as i32;
        let setting = &mut gene.inputs[index + 7];
        setting.source = input as i16;
        setting.mode = 1;
        setting.a = slope;
        setting.b = 0.0;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preset_matches_apk_channel_wiring_and_period() {
        let mut gene = crate::model::default_player_genome().remove(0);
        gene.cell_type = 13;
        apply_preset(&mut gene, 1, 0, 1, 2.0).unwrap();
        assert_eq!(&gene.output_modes[5..9], &[0, 1, 0, 1]);
        assert_eq!(gene.inputs[7].source, 1);
        assert_eq!(gene.inputs[8].source, 0);
        assert_eq!(gene.inputs[9].source, 0);
        assert_eq!(gene.inputs[10].source, 1);
        assert!(
            gene.inputs[7..=10]
                .iter()
                .all(|input| input.mode == 1 && input.b == 0.0)
        );
        assert!((gene.inputs[9].a - 11.5 / 20.0).abs() < f32::EPSILON);
        let period = measured_period(1, 11.5, gene.inputs[7].a * 20.0);
        assert!((period - 2.0).abs() < 0.02, "measured {period} hours");
    }

    #[test]
    fn preset_rejects_invalid_channels_without_mutating_gene() {
        let mut gene = crate::model::default_player_genome().remove(0);
        gene.cell_type = 13;
        let before = gene.inputs[7];
        assert!(apply_preset(&mut gene, 1, 0, 0, 2.0).is_err());
        assert_eq!(gene.inputs[7].a, before.a);
    }

    #[test]
    fn preset_tracks_short_and_long_periods_with_different_link_counts() {
        for (connections, desired) in [(0, 0.25), (2, 4.0), (5, 9.25)] {
            let mut gene = crate::model::default_player_genome().remove(0);
            gene.cell_type = 13;
            apply_preset(&mut gene, connections, 2, 3, desired).unwrap();
            let gain = 7.5 + connections as f32 * 4.0;
            let measured = measured_period(connections, gain, gene.inputs[7].a * 20.0);
            assert!(
                (measured - desired).abs() < 0.08,
                "{connections} links: requested {desired} hours, measured {measured}"
            );
        }
    }

    #[test]
    fn preset_drives_a_linked_flagellocyte_in_the_simulation() {
        let mut world = crate::legacy::import(crate::bundled::PLATES[0].1).unwrap();
        world.cells.truncate(2);
        for cell in &mut world.cells {
            cell.mode = 0;
            cell.u = [0.0; 4];
            cell.links.clear();
            cell.c = 0;
        }
        world.cells[0].genes[0].cell_type = 13;
        world.cells[1].genes[0].cell_type = 1;
        apply_preset(&mut world.cells[0].genes[0], 1, 0, 1, 1.0).unwrap();
        world.cells[0].u[0] = 1.0;
        for source in 0..2 {
            world.cells[source].links.push(crate::model::Link {
                target: (1 - source) as i32,
                c: 0.0,
                d: 0.0,
                b: false,
                e: 0.0,
                f: 0.0,
                k: 0.0,
                l: 0.0,
            });
            world.cells[source].c = 1;
        }
        for _ in 0..10 {
            crate::sensing::update_signals(&mut world, 0.02);
        }
        assert!(world.cells[1].u[0] > 0.01);
    }
}
