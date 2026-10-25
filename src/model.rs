use rand::Rng;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_CELL_ID: AtomicU64 = AtomicU64::new(1);

pub fn fresh_cell_id() -> u64 {
    NEXT_CELL_ID.fetch_add(1, Ordering::Relaxed)
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Environment {
    pub a: f64,
    pub b: f64,
    pub c: f64,
    pub d: f64,
    pub e: f64,
    pub f: f64,
    pub g: f64,
    pub h: f64,
    pub i: f64,
    pub j: f64,
    pub radius: f64,
    pub l: f64,
    pub m: f64,
    pub n: f32,
    pub o: f32,
    pub p: f64,
    pub q: f64,
    pub max_cells: usize,
    pub max_food: usize,
    pub enabled_types: Vec<bool>,
    pub u: bool,
    pub v: bool,
    pub w: bool,
    pub x: bool,
    pub y: bool,
    pub z: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Link {
    pub target: i32,
    pub c: f64,
    pub d: f64,
    pub b: bool,
    pub e: f64,
    pub f: f64,
    pub k: f32,
    pub l: f32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct GeneInput {
    pub source: i16,
    pub mode: i16,
    pub a: f32,
    pub b: f32,
    pub c: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Gene {
    pub color: [f32; 3],
    pub floats: [f32; 7],
    pub i: i32,
    pub j: i32,
    pub k: i32,
    pub flags: [bool; 7],
    pub cell_type: i32,
    pub inputs: [GeneInput; 11],
    pub output_modes: [i32; 11],
    pub output_values: [f32; 7],
}

/// One APK editor default mode, matching `i.c(int)`.
pub fn default_gene_for_mode(mode: usize, cell_type: i32) -> Gene {
    let mut rng = rand::rng();
    let mut color = [
        rng.random::<f32>(),
        rng.random::<f32>(),
        rng.random::<f32>(),
    ];
    let length = color
        .iter()
        .map(|channel| channel * channel)
        .sum::<f32>()
        .sqrt();
    for channel in &mut color {
        *channel /= length.max(f32::EPSILON);
    }
    let mut output_modes = [0; 11];
    output_modes[0] = mode as i32;
    output_modes[1] = mode as i32;
    output_modes[9] = 19;
    let mut inputs = [GeneInput::default(); 11];
    inputs[0].b = -1.0 + 10.0 * (2.0 / 24.0);
    let mut output_values = [0.0; 7];
    output_values[6] = 1.0;
    Gene {
        color,
        floats: [0.252, 0.5, 0.0, 0.0, 0.0, 1.0, 0.0],
        i: mode as i32,
        j: mode as i32,
        k: 0,
        flags: [false, true, true, true, mode == 0, false, false],
        cell_type,
        inputs,
        output_modes,
        output_values,
    }
}

/// Default 40-mode player genome, following the APK editor's i.c/b setup.
pub fn default_player_genome() -> Vec<Gene> {
    let mut genes = (0..40)
        .map(|mode| default_gene_for_mode(mode, 0))
        .collect::<Vec<_>>();
    genes[1].color[0] = 0.1;
    genes[2].color[0] = 0.9;
    genes[4].color[0] = 0.1;
    genes[4].color[1] = 0.9;
    genes[5].color[0] = 0.1;
    genes[6].color[0] = 0.5;
    genes
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Cell {
    #[serde(skip_serializing, skip_deserializing, default = "fresh_cell_id")]
    pub runtime_id: u64,
    pub x: f64,
    pub y: f64,
    pub values: [f64; 9],
    pub links: Vec<Link>,
    pub genes: Vec<Gene>,
    pub c: i32,
    pub x_flag: bool,
    pub color: [f32; 3],
    pub mode: i32,
    pub e: i32,
    pub f: i32,
    pub tuvp: [f64; 4],
    pub y_flag: bool,
    pub u: [f32; 4],
    pub w: [f32; 4],
    pub tail: [f32; 6],
    pub g: i32,
    pub t: i32,
    pub q: f64,
    #[serde(default)]
    pub resting: bool,
    #[serde(default)]
    pub visual_phase: f32,
    #[serde(default)]
    pub visual_phase2: f32,
    #[serde(skip)]
    pub mutation_ticks: u8,
}

impl Cell {
    pub fn active_gene(&self) -> Option<&Gene> {
        self.genes.get(self.mode.max(0) as usize)
    }

    pub fn cell_type(&self) -> i32 {
        self.active_gene().map(|gene| gene.cell_type).unwrap_or(0)
    }

    /// Matches Cell.a(output, 0) and Gene.x/y in the original APK.
    pub fn signal(&self, output: usize) -> f32 {
        let Some(gene) = self.active_gene() else {
            return 0.0;
        };
        let Some(input) = gene.inputs.get(output) else {
            return 0.0;
        };
        let source = match input.source {
            0..=3 => self.u[input.source as usize],
            4 => (self.values[7] / 0.36) as f32,
            5 => (self.values[8] / 240.0) as f32,
            6 => self.tuvp[3] as f32,
            7 => self.c as f32 / 20.0,
            _ => 0.0,
        };
        let value = match input.mode {
            0 => input.b,
            1 => source * input.a + input.b,
            _ if source < input.c => input.a,
            _ => input.b,
        }
        .clamp(-1.0, 1.0);
        let (lower, upper) = GENE_OUTPUT_RANGES
            .get(output)
            .copied()
            .unwrap_or((0.0, 0.0));
        lower + (value + 1.0) * (upper - lower) * 0.5
    }
}

/// Cell.a() output ranges from Gene.x/y in the original APK.
pub const GENE_OUTPUT_RANGES: [(f32, f32); 11] = [
    (0.0, 1.0),
    (-3.0, 3.0),
    (-1.0, 1.0),
    (-0.5, 0.5),
    (-1.0, 1.0),
    (-1.0, 1.0),
    (-1.0, 1.0),
    (-20.0, 20.0),
    (-20.0, 20.0),
    (-20.0, 20.0),
    (-20.0, 20.0),
];

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Food {
    pub x: f32,
    pub y: f32,
    pub energy: f32,
    pub vx: f32,
    pub vy: f32,
    pub state: f32,
}

#[derive(Clone, Debug)]
pub struct DeadCell {
    pub x: f32,
    pub y: f32,
    pub radius: f32,
    pub age: f32,
    pub color: [f32; 3],
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct World {
    pub source_version: Option<i32>,
    pub time: f64,
    pub light_angle: f64,
    pub environment: Environment,
    pub cells: Vec<Cell>,
    pub food: Vec<Food>,
    #[serde(skip)]
    pub dead: Vec<DeadCell>,
}

impl World {
    pub fn set_max_cells(&mut self, limit: usize) {
        self.environment.max_cells = limit;
    }

    pub fn set_max_food(&mut self, limit: usize) {
        self.environment.max_food = limit;
    }

    pub fn set_radius(&mut self, radius: f64) {
        self.environment.radius = radius.max(0.05);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apk_default_player_genome_has_forty_usable_phagocyte_modes() {
        let genes = default_player_genome();
        assert_eq!(genes.len(), 40);
        assert!(genes.iter().all(|gene| gene.cell_type == 0));
        assert_eq!(genes.iter().filter(|gene| gene.flags[4]).count(), 1);
        assert!(genes[0].flags[4]);
        for (index, gene) in genes.iter().enumerate() {
            assert_eq!(gene.i, index as i32);
            assert_eq!(gene.j, index as i32);
            assert_eq!(gene.floats[0], 0.252);
            assert_eq!(gene.output_modes[9], 19);
            assert_eq!(gene.output_values[6], 1.0);
        }
        let balloon = crate::levels::bundled()
            .into_iter()
            .find(|level| level.id == "30")
            .unwrap();
        assert!(balloon.allows_genes(&genes));
    }

    #[test]
    fn gene_signal_uses_mass_and_linear_input() {
        let mut world = crate::legacy::import(crate::bundled::PLATES[0].1).unwrap();
        let cell = &mut world.cells[0];
        cell.mode = 0;
        cell.values[7] = 0.18;
        cell.genes[0].inputs[0] = GeneInput {
            source: 4,
            mode: 1,
            a: 1.0,
            b: -1.0,
            c: 0.0,
        };
        assert!((cell.signal(0) - 0.25).abs() < 1.0e-6);
        cell.genes[0].inputs[0].mode = 0;
        cell.genes[0].inputs[0].b = 1.0;
        assert_eq!(cell.signal(0), 1.0);
    }

    #[test]
    fn four_neural_outputs_share_the_original_range() {
        let mut world = crate::legacy::import(crate::bundled::PLATES[0].1).unwrap();
        let cell = &mut world.cells[0];
        cell.mode = 0;
        for output in 7..11 {
            cell.genes[0].inputs[output].mode = 0;
            cell.genes[0].inputs[output].b = 1.0;
            assert_eq!(cell.signal(output), 20.0);
            cell.genes[0].inputs[output].b = -1.0;
            assert_eq!(cell.signal(output), -20.0);
        }
    }
}
