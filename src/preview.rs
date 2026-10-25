//! The APK's GenomePreviewView simulates a separate specimen from the edited
//! genome. Keep that work off the UI thread and cache a timeline for scrubbing.

use crate::{
    model::{Cell, Environment, Gene, World},
    sim::{self, Snapshot},
};
use rand::{SeedableRng, rngs::StdRng};
use std::{
    hash::{Hash, Hasher},
    sync::mpsc::{self, Receiver, Sender},
    thread,
};

struct Request {
    serial: u64,
    genes: Vec<Gene>,
}

struct Result {
    serial: u64,
    step: usize,
    snapshot: Snapshot,
}

pub struct PreviewEngine {
    requests: Sender<Request>,
    results: Receiver<Result>,
    gene_key: Option<u64>,
    serial: u64,
    target_step: usize,
    frames: Vec<(usize, Snapshot)>,
}

impl PreviewEngine {
    pub fn new() -> Self {
        let (requests, incoming) = mpsc::channel();
        let (outgoing, results) = mpsc::channel();
        thread::Builder::new()
            .name("cell-lab-genome-preview".into())
            .spawn(move || worker(incoming, outgoing))
            .expect("genome preview thread");
        Self {
            requests,
            results,
            gene_key: None,
            serial: 0,
            target_step: 0,
            frames: Vec::new(),
        }
    }

    pub fn request(&mut self, genes: &[Gene], time: f64) {
        if genes.is_empty() {
            self.frames.clear();
            self.gene_key = None;
            return;
        }
        self.target_step = (time.clamp(0.0, 48.0) / 0.02) as usize;
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        // Gene is serializable but contains floats, so hashing its serialized
        // bytes is simpler than maintaining a separate field-by-field key.
        if let Ok(bytes) = serde_json::to_vec(genes) {
            bytes.hash(&mut hasher);
        }
        let key = hasher.finish();
        if self.gene_key == Some(key) {
            return;
        }
        self.gene_key = Some(key);
        self.serial = self.serial.wrapping_add(1);
        self.frames.clear();
        self.frames
            .push((0, sim::preview_snapshot(&preview_world(genes.to_vec()))));
        let _ = self.requests.send(Request {
            serial: self.serial,
            genes: genes.to_vec(),
        });
    }

    pub fn poll(&mut self) {
        while let Ok(result) = self.results.try_recv() {
            if result.serial == self.serial {
                self.frames.push((result.step, result.snapshot));
            }
        }
    }

    pub fn snapshot(&self) -> Option<&Snapshot> {
        let index = self
            .frames
            .partition_point(|(step, _)| *step <= self.target_step)
            .saturating_sub(1);
        self.frames.get(index).map(|(_, snapshot)| snapshot)
    }

    pub fn current_frame_ready(&self) -> bool {
        self.frames
            .last()
            .is_some_and(|(step, _)| *step >= self.target_step)
    }
}

fn worker(incoming: Receiver<Request>, outgoing: Sender<Result>) {
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .thread_name(|_| "cell-lab-preview-worker".into())
        .build()
        .expect("preview simulation pool");
    while let Ok(mut request) = incoming.recv() {
        'compute: loop {
            while let Ok(next) = incoming.try_recv() {
                request = next;
            }
            let mut world = preview_world(request.genes.clone());
            let mut rng = StdRng::seed_from_u64(1337);
            for step in 1..=2400 {
                if step % 8 == 0 {
                    let mut replacement = None;
                    while let Ok(next) = incoming.try_recv() {
                        replacement = Some(next);
                    }
                    if let Some(next) = replacement {
                        request = next;
                        continue 'compute;
                    }
                }
                pool.install(|| sim::step_preview(&mut world, 0.02, &mut rng));
                // The first six hours have finer slider resolution. Later
                // frames can be sampled more sparsely without visible jumps.
                if (step <= 300 && step % 2 == 0) || (step > 300 && step % 5 == 0) {
                    if outgoing
                        .send(Result {
                            serial: request.serial,
                            step,
                            snapshot: sim::preview_snapshot(&world),
                        })
                        .is_err()
                    {
                        return;
                    }
                }
            }
            break;
        }
    }
}

fn preview_world(genes: Vec<Gene>) -> World {
    let mode = genes.iter().rposition(|gene| gene.flags[4]).unwrap_or(0);
    let color = genes[mode].color;
    let reserve = if genes[mode].cell_type == 4 { 1.8 } else { 0.0 };
    let cell = Cell {
        runtime_id: crate::model::fresh_cell_id(),
        x: 0.0,
        y: 0.0,
        values: [
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            (0.288_f64 / 400.0).sqrt(),
            0.288,
            0.0,
        ],
        links: Vec::new(),
        genes,
        c: 0,
        x_flag: false,
        color,
        mode: mode as i32,
        e: 0,
        f: 0,
        tuvp: [0.0, 0.0, 0.0, 1.0],
        y_flag: false,
        u: [0.01; 4],
        w: [0.0; 4],
        tail: [0.0, 0.0, 0.0, 0.0, 0.0, reserve],
        g: 0,
        t: -1,
        q: 0.0,
        resting: false,
        visual_phase: 0.0,
        visual_phase2: 0.0,
        mutation_ticks: 0,
    };
    World {
        source_version: None,
        time: 0.0,
        light_angle: 0.0,
        environment: Environment {
            a: 0.0,
            b: 0.0,
            c: 0.0,
            d: 0.0,
            e: 0.0,
            f: 0.0,
            g: 75.0,
            h: 0.0,
            i: 0.0,
            j: 0.0,
            radius: 1.0,
            l: 0.0,
            m: 0.0,
            n: 0.0,
            o: 0.0,
            p: 0.0,
            q: 0.0,
            max_cells: 128,
            max_food: 0,
            enabled_types: vec![false; 18],
            u: false,
            v: false,
            w: false,
            x: true,
            y: false,
            z: 1.0,
        },
        cells: vec![cell],
        food: Vec::new(),
        dead: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_starts_from_the_marked_gene_and_can_divide() {
        let mut genes = crate::model::default_player_genome();
        genes[0].flags[4] = false;
        genes[1].flags[4] = true;
        genes[1].flags[0] = true;
        genes[1].cell_type = 0;
        let mut world = preview_world(genes);
        let mut rng = StdRng::seed_from_u64(1337);
        assert_eq!(world.cells[0].mode, 1);
        assert_eq!(world.cells[0].values[7], 0.288);
        assert!((world.cells[0].values[6] - (0.288_f64 / 400.0).sqrt()).abs() < 1.0e-12);
        for _ in 0..40 {
            sim::step_preview(&mut world, 0.02, &mut rng);
        }
        assert!(world.cells.len() > 1);
        assert!(!sim::preview_snapshot(&world).links.is_empty());
    }

    #[test]
    fn default_genome_preview_has_a_cell_at_two_seconds() {
        let mut world = preview_world(crate::model::default_player_genome());
        let mut rng = StdRng::seed_from_u64(1337);
        for _ in 0..100 {
            sim::step_preview(&mut world, 0.02, &mut rng);
        }
        assert!(!world.cells.is_empty());
        assert!(!sim::preview_snapshot(&world).cells.is_empty());
    }

    #[test]
    fn seeded_preview_repeats_random_contact_outcomes() {
        let mut first = preview_world(crate::model::default_player_genome());
        let mut victim = first.cells[0].clone();
        victim.x = 0.02;
        first.cells[0].genes[0].cell_type = 8;
        first.cells.push(victim);
        let mut second = first.clone();
        let mut first_rng = StdRng::seed_from_u64(1337);
        let mut second_rng = StdRng::seed_from_u64(1337);
        for _ in 0..10 {
            first.cells[0].x = 0.0;
            first.cells[1].x = 0.02;
            second.cells[0].x = 0.0;
            second.cells[1].x = 0.02;
            sim::step_preview(&mut first, 0.02, &mut first_rng);
            sim::step_preview(&mut second, 0.02, &mut second_rng);
        }
        assert_eq!(
            serde_json::to_vec(&first).unwrap(),
            serde_json::to_vec(&second).unwrap()
        );
    }

    #[test]
    fn timeline_scrubbing_reuses_frames_without_clearing_the_preview() {
        let genes = crate::model::default_player_genome();
        let mut preview = PreviewEngine::new();
        preview.request(&genes, 0.0);
        assert!(preview.snapshot().is_some());
        preview.request(&genes, 1.0);
        assert!(preview.snapshot().is_some());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while !preview.current_frame_ready() {
            assert!(
                std::time::Instant::now() < deadline,
                "preview timeline stalled"
            );
            preview.poll();
            thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(preview.snapshot().unwrap().time >= 0.96);
        preview.request(&genes, 0.4);
        assert!(preview.current_frame_ready());
        assert!((preview.snapshot().unwrap().time - 0.4).abs() < 0.041);
        preview.request(&genes, 1.0);
        assert!(preview.current_frame_ready());
    }
}
