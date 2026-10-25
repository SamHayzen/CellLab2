use crate::{
    bundled::PLATES,
    legacy,
    model::{Gene, World},
    sim::Snapshot,
};
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Cell,
    Move,
    Boost,
    Remove,
    Inspect,
}

impl Tool {
    pub fn label(self) -> &'static str {
        match self {
            Self::Cell => "Cell",
            Self::Move => "Move",
            Self::Boost => "Boost",
            Self::Remove => "Remove",
            Self::Inspect => "Inspect",
        }
    }
}

pub const TOOLS: [Tool; 5] = [
    Tool::Cell,
    Tool::Move,
    Tool::Boost,
    Tool::Remove,
    Tool::Inspect,
];

pub struct Level {
    pub id: String,
    pub plate: String,
    pub title: String,
    pub description: String,
    pub hint: String,
    pub tip: String,
    pub difficulty: i32,
    pub place_limit: usize,
    pub remove_limit: Option<usize>,
    pub conditions: Vec<Condition>,
    pub hold_time: f64,
    pub tools: [bool; 5],
    pub max_gene_types: [Option<usize>; 18],
    pub allowed_types: [bool; 18],
}

impl Level {
    pub fn allows(&self, tool: Tool) -> bool {
        self.tools[tool as usize]
    }

    pub fn allows_genes(&self, genes: &[Gene]) -> bool {
        self.gene_limits().iter().enumerate().all(|(kind, limit)| {
            limit.is_none_or(|max| {
                genes
                    .iter()
                    .filter(|gene| gene.cell_type == kind as i32)
                    .count()
                    <= max
            })
        })
    }

    pub fn gene_limits(&self) -> [Option<usize>; 18] {
        std::array::from_fn(|kind| {
            if self.allowed_types[kind] {
                self.max_gene_types[kind]
            } else {
                Some(0)
            }
        })
    }

    pub fn starting_genome(&self) -> Vec<Gene> {
        let mut genes = crate::model::default_player_genome();
        self.adapt_genome(&mut genes);
        genes
    }

    pub fn adapt_genome(&self, genes: &mut [Gene]) -> bool {
        let Some(first) = self.allowed_types.iter().position(|enabled| *enabled) else {
            return false;
        };
        let mut changed = false;
        for gene in genes {
            if !self
                .allowed_types
                .get(gene.cell_type as usize)
                .copied()
                .unwrap_or(false)
            {
                gene.cell_type = first as i32;
                changed = true;
            }
        }
        changed
    }
}

#[derive(Clone)]
pub struct Condition {
    pub tag: String,
    pub target: f64,
}

pub struct TutorialStep {
    pub title: &'static str,
    pub text: &'static str,
}

const MICROSCOPE_TUTORIAL: &[TutorialStep] = &[
    TutorialStep {
        title: "The three work areas",
        text: "The left panel holds plates and challenges, the microscope is in the center, and the genome editor is on the right. This landscape layout keeps all three areas visible at once.",
    },
    TutorialStep {
        title: "Zoom and navigate",
        text: "Scroll over the microscope to zoom. Drag empty space to pan around the Petri dish. Find the lighter area where sunlight reaches the plate.",
    },
    TutorialStep {
        title: "Place an organism",
        text: "Choose Cell above the microscope, then click inside the plate. The new organism uses the synthesizer genome shown on the right. Place it in the light and watch what happens.",
    },
    TutorialStep {
        title: "Run the plate",
        text: "Use Play and Step above the microscope to advance time. Enter a speed or choose Incubate to run as fast as the computer allows. More cells can grow from the organism you placed.",
    },
    TutorialStep {
        title: "Inspect and copy",
        text: "Choose Inspect and click an organism. Its details appear in the Genome panel. Copy selected genome to synthesizer to use that organism as your next starting design.",
    },
    TutorialStep {
        title: "Save and clear",
        text: "Use Save to keep this experiment as a .celllab2 or .substrate plate. Clear starts a new empty plate. Save before clearing if you want to return to your organisms.",
    },
    TutorialStep {
        title: "Microscope training complete",
        text: "You can now navigate, place, run, inspect, and save a plate. Finish this tutorial when you are ready to explore genome editing.",
    },
];

const GENOME_TUTORIAL: &[TutorialStep] = &[
    TutorialStep {
        title: "The genome panel",
        text: "The right panel edits the synthesizer genome. Its Cell Preview shows a separate organism made from that genome. Drag the Time slider to inspect different ages.",
    },
    TutorialStep {
        title: "Forty gene modes",
        text: "A genome has 40 modes. Change Gene mode in the Synthesizer Genome section to edit a different mode. Each mode can have its own cell type, color, division rule, and actions.",
    },
    TutorialStep {
        title: "Starting mode",
        text: "Choose a mode and press Initial mode. The preview and newly placed organisms will begin in that mode.",
    },
    TutorialStep {
        title: "Color and type",
        text: "Choose a cell type and set its Gene color. The preview shows how that mode looks with the APK cell shader. Try making one mode red and another a different color.",
    },
    TutorialStep {
        title: "Division and links",
        text: "Open Division and links. Parent mode and Daughter mode choose the modes after a split. Link parent and daughter makes the new cells stay attached. Advance preview time to see the result.",
    },
    TutorialStep {
        title: "Review the life cycle",
        text: "Switch between the modes in your genome and review their cell types, colors, and division paths. The Cell Preview lets you check the result before you place the organism. Programmable signals are available from the start.",
    },
    TutorialStep {
        title: "Try your design",
        text: "Choose Cell, click the microscope plate, and run the simulation. Save synthesizer genome keeps the design for the next session; Save stores the entire plate.",
    },
    TutorialStep {
        title: "Genome training complete",
        text: "You can now edit modes, preview division, and place your own genome. Finish this tutorial to mark it complete.",
    },
];

const SWIMMER_TUTORIAL: &[TutorialStep] = &[
    TutorialStep {
        title: "Plan a swimmer",
        text: "A simple swimmer has a Flagellocyte for propulsion and a Phagocyte for feeding. Start with a Phagocyte mode that can divide into those two modes.",
    },
    TutorialStep {
        title: "Connect the daughters",
        text: "In the starting mode, open Division and links. Set Parent mode and Daughter mode to your propulsion and feeding modes, and enable Link parent and daughter.",
    },
    TutorialStep {
        title: "Make propulsion",
        text: "Edit the propulsion mode and choose Flagellocyte. Open Cell actions and raise Swim force. A positive value pushes the cell in its facing direction.",
    },
    TutorialStep {
        title: "Keep the feeder alive",
        text: "Edit the feeding mode and choose Phagocyte. It gains mass by consuming food. Set its division modes to recreate your starting mode so the life cycle can continue.",
    },
    TutorialStep {
        title: "Preview the family",
        text: "Move the preview Time slider forward to watch division and links. Adjust the division angles and child modes until the linked pair forms the shape you want.",
    },
    TutorialStep {
        title: "Test and save",
        text: "Place the starting cell on a plate with food, press Play, and watch the swimmer move and reproduce. Save the synthesizer genome when you are satisfied with the design.",
    },
];

const SIGNAL_TUTORIAL: &[TutorialStep] = &[
    TutorialStep {
        title: "Four signal channels",
        text: "Cells can hold four signal substances. Neurocytes produce them, while programmable actions can read them. Links carry signals between connected cells.",
    },
    TutorialStep {
        title: "Build a signaling organism",
        text: "Make a starting Neurocyte that divides into a Flagellocyte and a feeding cell. Enable Link parent and daughter so the cells can pass signals through the organism.",
    },
    TutorialStep {
        title: "Program swim force",
        text: "In the Flagellocyte mode, open Programmed signals and Output 0. Choose Signal 1 as the input source and Linear as the program. Try a positive Slope and zero Bias.",
    },
    TutorialStep {
        title: "Produce Signal 1",
        text: "In the Neurocyte mode, choose Signal 1 for a Neural channel under Gene modes and cell actions. Raise its matching output under Programmed signals to make the cell produce S1.",
    },
    TutorialStep {
        title: "Watch signal decay",
        text: "Place the starting cell and press Play. A Flagellocyte swims while it receives S1, then slows as the signal decays. Adjust production and connections to sustain movement.",
    },
    TutorialStep {
        title: "Oscillator preset",
        text: "Change the feeding cell to a Neurocyte. Open Programmed signals and Oscillator preset, choose one adhesin connection, channels S1 and S2, and a one-hour period. Apply it, place a new organism, and watch the linked Flagellocyte swim in pulses.",
    },
];

pub fn tutorial_steps(id: &str) -> Option<&'static [TutorialStep]> {
    match id {
        "tuto0" => Some(MICROSCOPE_TUTORIAL),
        "tuto1" => Some(GENOME_TUTORIAL),
        "tuto2" => Some(SWIMMER_TUTORIAL),
        "tuto3" => Some(SIGNAL_TUTORIAL),
        _ => None,
    }
}

const CONDITION_TAGS: [&str; 14] = [
    "user-cells",
    "user-cells-max",
    "total-cells-min",
    "level-cells",
    "level-cells-min",
    "red-level-cells",
    "red-level-cells-min",
    "yellow-level-cells",
    "yellow-level-cells-min",
    "black-level-cells",
    "infected-level-cells",
    "infected-level-cells-max",
    "hybrid-cells-min",
    "max-time",
];

fn field<'a>(source: &'a str, tag: &str) -> Option<&'a str> {
    let start = format!("<{tag}>");
    let end = format!("</{tag}>");
    source
        .split_once(&start)?
        .1
        .split_once(&end)
        .map(|(value, _)| value.trim())
}

pub fn bundled() -> Vec<Level> {
    let source = include_str!("../assets/levels.xml");
    let texts: Vec<String> = serde_json::from_str(include_str!("../assets/level_texts.json"))
        .expect("APK challenge text asset");
    let mut allowed_by_plate: HashMap<String, [bool; 18]> = HashMap::new();
    source
        .split("<level>")
        .skip(1)
        .enumerate()
        .filter_map(|(index, chunk)| {
            let (content, _) = chunk.split_once("</level>")?;
            let id = field(content, "id")?.to_owned();
            let plate = field(content, "file")?
                .trim_end_matches(".substrate")
                .to_owned();
            let conditions = CONDITION_TAGS
                .iter()
                .filter_map(|tag| {
                    Some(Condition {
                        tag: (*tag).into(),
                        target: field(content, tag)?.parse().ok()?,
                    })
                })
                .collect();
            let tools = match field(content, "tools") {
                Some(raw) if !raw.is_empty() => {
                    let values: Vec<_> = raw.split(',').map(str::trim).collect();
                    ["cell", "move", "add", "remove", "info"].map(|name| values.contains(&name))
                }
                _ => [true, false, false, false, true],
            };
            let cell_types = [
                "phagocyte",
                "flagellocyte",
                "photocyte",
                "devorocyte",
                "lipocyte",
                "keratinocyte",
                "buoyocyte",
                "glueocyte",
                "virocyte",
                "nitrocyte",
                "stereocyte",
                "senseocyte",
                "myocyte",
                "neurocyte",
                "secrocyte",
                "stemocyte",
                "gamete",
                "ciliocyte",
            ];
            let max_gene_types = cell_types.map(|name| {
                field(content, &format!("max-{name}")).and_then(|value| value.parse().ok())
            });
            let allowed_types = if let Some(types) = allowed_by_plate.get(&plate) {
                *types
            } else {
                let (_, bytes) = PLATES.iter().find(|(name, _)| *name == plate)?;
                let world = legacy::import(bytes).ok()?;
                let types = std::array::from_fn(|kind| {
                    world
                        .environment
                        .enabled_types
                        .get(kind)
                        .copied()
                        .unwrap_or(false)
                });
                allowed_by_plate.insert(plate.clone(), types);
                types
            };
            Some(Level {
                id,
                plate,
                title: texts.get(index * 4).cloned().unwrap_or_default(),
                description: texts.get(index * 4 + 1).cloned().unwrap_or_default(),
                hint: texts.get(index * 4 + 2).cloned().unwrap_or_default(),
                tip: texts.get(index * 4 + 3).cloned().unwrap_or_default(),
                difficulty: field(content, "difficulty")
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0),
                place_limit: field(content, "place-cells")
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(1),
                remove_limit: field(content, "remove-max").and_then(|s| s.parse().ok()),
                conditions,
                hold_time: field(content, "win-time")
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0.0),
                tools,
                max_gene_types,
                allowed_types,
            })
        })
        .collect()
}

pub fn condition_label(tag: &str) -> &str {
    match tag {
        "user-cells" => "Your organisms ≥",
        "user-cells-max" => "Your organisms ≤",
        "total-cells-min" => "Total organisms ≥",
        "level-cells" => "Native organisms ≤",
        "level-cells-min" => "Native organisms ≥",
        "red-level-cells" => "Red natives ≤",
        "red-level-cells-min" => "Red natives ≥",
        "yellow-level-cells" => "Yellow natives ≤",
        "yellow-level-cells-min" => "Yellow natives ≥",
        "black-level-cells" => "Black natives ≤",
        "infected-level-cells" => "Infected natives ≥",
        "infected-level-cells-max" => "Infected natives ≤",
        "hybrid-cells-min" => "Hybrid organisms ≥",
        "max-time" => "Time ≤",
        _ => "Goal",
    }
}

pub fn condition_met(condition: &Condition, snapshot: &Snapshot) -> bool {
    GoalCounts::from_cells(
        snapshot
            .cells
            .iter()
            .map(|cell| (cell.origin, cell.stored_color)),
        snapshot.time,
    )
    .met(condition)
}

pub fn conditions_met_world(conditions: &[Condition], world: &World) -> bool {
    let counts = GoalCounts::from_cells(
        world.cells.iter().map(|cell| (cell.e, cell.color)),
        world.time,
    );
    conditions.iter().all(|condition| counts.met(condition))
}

struct GoalCounts {
    origins: [f64; 7],
    total: f64,
    red: f64,
    yellow: f64,
    black: f64,
    time: f64,
}

impl GoalCounts {
    fn from_cells(cells: impl Iterator<Item = (i32, [f32; 3])>, time: f64) -> Self {
        let mut counts = Self {
            origins: [0.0; 7],
            total: 0.0,
            red: 0.0,
            yellow: 0.0,
            black: 0.0,
            time,
        };
        for (origin, color) in cells {
            counts.total += 1.0;
            if let Some(count) = counts.origins.get_mut(origin as usize) {
                *count += 1.0;
            }
            if origin == 0 {
                if color[0] > 0.65 && color[1] < 0.35 && color[2] < 0.35 {
                    counts.red += 1.0;
                }
                if color[0] > 0.65 && color[1] > 0.65 && color[2] < 0.35 {
                    counts.yellow += 1.0;
                }
                if color.iter().map(|v| v.abs()).sum::<f32>() < 0.1 {
                    counts.black += 1.0;
                }
            }
        }
        counts
    }

    fn met(&self, condition: &Condition) -> bool {
        let t = condition.target;
        let natives = self.origins[0] + self.origins[3];
        match condition.tag.as_str() {
            "user-cells" => self.origins[1] >= t,
            "user-cells-max" => self.origins[1] <= t,
            "total-cells-min" => self.total >= t,
            "level-cells" => natives <= t,
            "level-cells-min" => natives >= t,
            "red-level-cells" => self.red <= t,
            "red-level-cells-min" => self.red >= t,
            "yellow-level-cells" => self.yellow <= t,
            "yellow-level-cells-min" => self.yellow >= t,
            "black-level-cells" => self.black <= t,
            "infected-level-cells" => self.origins[3] >= t,
            "infected-level-cells-max" => self.origins[3] <= t,
            "hybrid-cells-min" => self.origins[6] >= t,
            "max-time" => self.time <= t,
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tutorial_levels_have_desktop_guides() {
        let levels = bundled();
        for id in ["tuto0", "tuto1", "tuto2", "tuto3"] {
            assert!(levels.iter().any(|level| level.id == id));
            let steps = tutorial_steps(id).unwrap();
            assert!(steps.len() >= 5);
            assert!(
                steps
                    .iter()
                    .all(|step| !step.title.is_empty() && !step.text.is_empty())
            );
        }
        assert!(tutorial_steps("1").is_none());
    }

    #[test]
    fn bundled_levels_have_valid_plates() {
        let levels = bundled();
        assert_eq!(levels.len(), 70);
        assert_eq!(levels[2].title, "Algae");
        assert!(levels[2].description.contains("100 cells"));
        for level in levels {
            assert!(
                crate::bundled::PLATES
                    .iter()
                    .any(|(name, _)| *name == level.plate)
            );
        }
    }

    #[test]
    fn challenge_tools_and_gene_limits_match_level_data() {
        let levels = bundled();
        let boost_only = levels.iter().find(|level| level.id == "3.3").unwrap();
        assert!(boost_only.allows(Tool::Boost));
        assert!(!boost_only.allows(Tool::Cell));
        assert!(!boost_only.allows(Tool::Inspect));
        let balloon = levels.iter().find(|level| level.id == "30").unwrap();
        assert_eq!(balloon.max_gene_types[6], Some(1));
        assert!(
            balloon
                .conditions
                .iter()
                .all(|condition| condition.tag != "max-buoyocyte")
        );
        let world = crate::legacy::import(crate::bundled::PLATES[0].1).unwrap();
        let mut genes = world.cells[0].genes.clone();
        for gene in &mut genes {
            gene.cell_type = 0;
        }
        assert!(balloon.allows_genes(&genes));
        genes[0].cell_type = 6;
        assert!(balloon.allows_genes(&genes));
        genes[1].cell_type = 6;
        assert!(!balloon.allows_genes(&genes));
    }

    #[test]
    fn challenge_plate_controls_available_gene_types() {
        let levels = bundled();
        assert!(
            levels
                .iter()
                .all(|level| level.allowed_types.contains(&true))
        );
        let level = levels
            .iter()
            .find(|level| level.allowed_types.contains(&false))
            .unwrap();
        let available = level.allowed_types.iter().position(|value| *value).unwrap();
        let locked = level
            .allowed_types
            .iter()
            .position(|value| !*value)
            .unwrap();
        let mut genes = level.starting_genome();
        assert!(
            genes
                .iter()
                .all(|gene| level.allowed_types[gene.cell_type as usize])
        );
        genes[0].cell_type = locked as i32;
        assert!(!level.allows_genes(&genes));
        assert_eq!(level.gene_limits()[locked], Some(0));
        genes[0].cell_type = available as i32;
        assert!(!level.adapt_genome(&mut genes));
    }

    #[test]
    fn every_placeable_challenge_starts_with_a_legal_genome() {
        for level in bundled() {
            if level.allows(Tool::Cell) && level.place_limit > 0 {
                let genes = level.starting_genome();
                assert!(
                    level.allows_genes(&genes),
                    "{} starts with an illegal genome",
                    level.id
                );
            }
        }
    }

    #[test]
    fn removal_cap_is_a_tool_limit_not_a_win_condition() {
        let levels = bundled();
        let maze = levels.iter().find(|level| level.id == "22.5").unwrap();
        assert_eq!(maze.remove_limit, Some(14));
        assert!(
            maze.conditions
                .iter()
                .all(|condition| condition.tag != "remove-max")
        );
        assert!(
            maze.conditions
                .iter()
                .any(|condition| condition.tag == "yellow-level-cells-min")
        );
    }
}
