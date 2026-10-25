use crate::{
    audio::{Audio, Sound, Track},
    bundled, legacy,
    levels::{self, Level, Tool},
    model::{
        Cell, Environment, GENE_OUTPUT_RANGES, Gene, World, default_gene_for_mode,
        default_player_genome,
    },
    oscillator,
    preview::PreviewEngine,
    progress::Progress,
    sim::{self, Command, Engine, Event, Snapshot},
    visuals::Icons,
};
use eframe::egui::{self, Color32, Pos2, Sense, Stroke, Vec2};
use std::path::PathBuf;
use std::time::Duration;

struct Challenge {
    run_id: u64,
    index: usize,
    tutorial_step: usize,
    placed: usize,
    pending_places: usize,
    removed: usize,
    pending_removals: usize,
    won: bool,
    pending: bool,
}

struct OscillatorControls {
    connections: usize,
    first_channel: usize,
    second_channel: usize,
    period: f32,
    show_preset: bool,
    programmed_output: Option<usize>,
}

impl Default for OscillatorControls {
    fn default() -> Self {
        Self {
            connections: 1,
            first_channel: 0,
            second_channel: 1,
            period: 2.0,
            show_preset: false,
            programmed_output: None,
        }
    }
}

fn zoom_at_pointer(
    zoom: &mut f32,
    pan: &mut Vec2,
    viewport_center: Pos2,
    pointer: Pos2,
    scroll_delta: f32,
) {
    let old_zoom = *zoom;
    let next_zoom = (old_zoom * (1.0 + scroll_delta * 0.001)).clamp(0.2, 30.0);
    let center = viewport_center + *pan;
    *pan += (pointer - center) * (1.0 - next_zoom / old_zoom);
    *zoom = next_zoom;
}

pub struct App {
    engine: Engine,
    snapshot: Snapshot,
    selected: Option<usize>,
    selected_id: Option<u64>,
    source_name: String,
    open_path: String,
    save_path: String,
    genome_path: String,
    show_genome_load: bool,
    status: String,
    draft_radius: f64,
    draft_cells: usize,
    draft_food_limit: usize,
    draft_food_count: usize,
    draft_environment: Environment,
    show_environment_settings: bool,
    zoom: f32,
    pan: Vec2,
    filter: String,
    refresh_inputs: bool,
    levels: Vec<Level>,
    challenge: Option<Challenge>,
    next_challenge_run_id: u64,
    show_challenges: bool,
    inspected: Option<Cell>,
    gene_mode: usize,
    blueprint_genes: Vec<Gene>,
    blueprint_gene_mode: usize,
    snap_gene_sliders: bool,
    oscillator: OscillatorControls,
    edit_blueprint: bool,
    preview_progress: f64,
    speed_input: f64,
    preview: PreviewEngine,
    active_tool: Tool,
    moving: Option<u64>,
    progress: Progress,
    audio: Option<Audio>,
    icons: Icons,
    cell_format: eframe::wgpu::TextureFormat,
    capture_path: Option<PathBuf>,
    capture_requested: bool,
}

impl App {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        capture_path: Option<PathBuf>,
        capture_plate: Option<String>,
        capture_zoom: Option<f32>,
        capture_pan_y: Option<f32>,
        capture_preview_time: Option<f64>,
    ) -> Self {
        let mut visuals = egui::Visuals::dark();
        visuals.panel_fill = Color32::from_rgb(32, 32, 32);
        visuals.window_fill = Color32::from_rgb(48, 48, 48);
        visuals.selection.bg_fill = Color32::from_rgb(56, 129, 11);
        visuals.selection.stroke = Stroke::new(1.0, Color32::from_rgb(255, 170, 0));
        cc.egui_ctx.set_visuals(visuals);
        let icons = Icons::load(&cc.egui_ctx);
        let cell_format = cc
            .wgpu_render_state
            .as_ref()
            .expect("WGPU renderer")
            .target_format;
        let (initial_name, initial_bytes) = capture_plate
            .as_deref()
            .and_then(|name| {
                bundled::PLATES
                    .iter()
                    .find(|(candidate, _)| *candidate == name)
            })
            .copied()
            .unwrap_or(bundled::PLATES[0]);
        let mut world = legacy::import(initial_bytes).expect("bundled starter plate");
        if capture_plate.as_deref() == Some("__effects__") {
            if let Some(cell) = world.cells.first() {
                world.dead.push(crate::model::DeadCell {
                    x: cell.x as f32 + 0.08,
                    y: cell.y as f32,
                    radius: cell.values[6] as f32,
                    age: 0.15,
                    color: cell.color,
                });
            }
        }
        let snapshot = snapshot_for_start(&world);
        let draft_radius = world.environment.radius;
        let draft_cells = world.environment.max_cells;
        let draft_food_limit = world.environment.max_food;
        let draft_food_count = world.food.len();
        let draft_environment = world.environment.clone();
        let (mut progress, mut status) = match Progress::load() {
            Ok(progress) => (progress, "Ready".to_owned()),
            Err(error) => (
                Progress::default(),
                format!("Progress could not be read: {error}"),
            ),
        };
        if capture_plate.as_deref() == Some("__unlocked__") {
            progress
                .completed
                .extend(["11".into(), "16".into(), "26.6".into()]);
        }
        let audio = if capture_path.is_some() {
            None
        } else {
            match Audio::new(progress.music_enabled, progress.effects_enabled) {
                Ok(audio) => Some(audio),
                Err(error) => {
                    status = format!("{status}. Audio unavailable: {error}");
                    None
                }
            }
        };
        let mut blueprint_genes = progress
            .blueprint
            .clone()
            .filter(|genes| !genes.is_empty())
            .unwrap_or_else(default_player_genome);
        if !blueprint_genes.iter().any(|gene| gene.flags[4]) {
            if let Some(gene) = blueprint_genes.get_mut(0) {
                gene.flags[4] = true;
            }
        }
        if capture_plate.as_deref() == Some("__effects__") {
            if let Some(mode) = blueprint_genes.iter().rposition(|gene| gene.flags[4]) {
                blueprint_genes[mode].flags[0] = true;
            }
        }
        let capture_inspected = (capture_plate.as_deref() == Some("__inspector__"))
            .then(|| world.cells.first().cloned())
            .flatten();
        let capture_selected_id = capture_inspected.as_ref().map(|cell| cell.runtime_id);
        let capture_gene_mode = capture_inspected
            .as_ref()
            .map_or(0, |cell| cell.mode.max(0) as usize);
        let mut app = Self {
            engine: sim::start(world),
            snapshot,
            selected: capture_selected_id.map(|_| 0),
            selected_id: capture_selected_id,
            source_name: initial_name.into(),
            open_path: String::new(),
            save_path: "my-plate.celllab2".into(),
            genome_path: "my-genome.genome".into(),
            show_genome_load: false,
            status,
            draft_radius,
            draft_cells,
            draft_food_limit,
            draft_food_count,
            draft_environment,
            show_environment_settings: capture_plate.as_deref() == Some("__environment__"),
            zoom: capture_zoom.unwrap_or(1.0).clamp(0.1, 30.0),
            pan: Vec2::new(0.0, capture_pan_y.unwrap_or(0.0)),
            filter: String::new(),
            refresh_inputs: false,
            levels: levels::bundled(),
            challenge: None,
            next_challenge_run_id: 0,
            show_challenges: false,
            inspected: capture_inspected,
            gene_mode: capture_gene_mode,
            blueprint_genes,
            blueprint_gene_mode: 0,
            snap_gene_sliders: true,
            oscillator: OscillatorControls::default(),
            edit_blueprint: capture_selected_id.is_none(),
            speed_input: 1.0,
            preview_progress: capture_preview_time.map_or(0.0, |time| {
                let time = time.clamp(0.0, 48.0);
                if time <= 6.0 {
                    time / 12.0
                } else {
                    0.5 + (time - 6.0) / 84.0
                }
            }),
            preview: PreviewEngine::new(),
            active_tool: Tool::Inspect,
            moving: None,
            progress,
            audio,
            icons,
            cell_format,
            capture_path,
            capture_requested: false,
        };
        if let Some(id) = capture_plate
            .as_deref()
            .and_then(|name| name.strip_prefix("challenge:"))
        {
            if let Some(index) = app.levels.iter().position(|level| level.id == id) {
                app.open_challenge(index);
            }
        }
        app
    }

    fn send(&self, command: Command) {
        let _ = self.engine.commands.send(command);
    }

    fn play_sound(&self, sound: Sound) {
        if let Some(audio) = &self.audio {
            audio.play(sound);
        }
    }

    fn tool_allowed(&self, tool: Tool) -> bool {
        self.challenge
            .as_ref()
            .is_none_or(|challenge| self.levels[challenge.index].allows(tool))
    }

    fn unlocked_features(&self) -> [bool; 3] {
        [true; 3]
    }

    fn can_remove(&self) -> bool {
        self.tool_allowed(Tool::Remove)
            && self.challenge.as_ref().is_none_or(|challenge| {
                self.levels[challenge.index]
                    .remove_limit
                    .is_none_or(|limit| challenge.removed + challenge.pending_removals < limit)
            })
    }

    fn poll(&mut self) {
        while let Ok(event) = self.engine.events.try_recv() {
            match event {
                Event::Snapshot(snapshot) => {
                    if self.refresh_inputs {
                        self.draft_radius = snapshot.radius;
                        self.draft_cells = snapshot.max_cells;
                        self.draft_food_limit = snapshot.max_food;
                        self.draft_food_count = snapshot.food.len();
                        self.draft_environment = snapshot.environment.clone();
                        self.refresh_inputs = false;
                        self.selected = None;
                        self.selected_id = None;
                        self.inspected = None;
                        self.pan = Vec2::ZERO;
                        self.zoom = 1.0;
                    }
                    if let Some(id) = self.selected_id {
                        if self.selected.is_none_or(|index| {
                            snapshot
                                .cells
                                .get(index)
                                .is_none_or(|cell| cell.runtime_id != id)
                        }) {
                            self.selected =
                                snapshot.cells.iter().position(|cell| cell.runtime_id == id);
                        }
                        if self.selected.is_none() {
                            self.selected_id = None;
                            self.inspected = None;
                        }
                    }
                    self.snapshot = snapshot;
                    self.engine.acknowledge_snapshot();
                }
                Event::ChallengeWon(run_id) => {
                    if self
                        .challenge
                        .as_ref()
                        .is_some_and(|challenge| challenge.run_id == run_id)
                    {
                        self.complete_challenge();
                    }
                }
                Event::Message(message) => {
                    if message.starts_with("Saved ") {
                        self.play_sound(Sound::Save);
                    }
                    self.status = message;
                }
                Event::Loaded => {
                    self.refresh_inputs = true;
                    self.inspected = None;
                    self.moving = None;
                    if let Some(challenge) = &mut self.challenge {
                        challenge.pending = false;
                    }
                }
                Event::CellAdded => {
                    self.play_sound(Sound::Place);
                    if let Some(challenge) = &mut self.challenge {
                        challenge.pending_places = challenge.pending_places.saturating_sub(1);
                        challenge.placed += 1;
                    }
                }
                Event::CellAddRejected => {
                    self.play_sound(Sound::Invalid);
                    if let Some(challenge) = &mut self.challenge {
                        challenge.pending_places = challenge.pending_places.saturating_sub(1);
                    }
                }
                Event::CellRemoved => {
                    self.play_sound(Sound::Sterilize);
                    if let Some(challenge) = &mut self.challenge {
                        challenge.pending_removals = challenge.pending_removals.saturating_sub(1);
                        challenge.removed += 1;
                    }
                }
                Event::CellRemoveRejected => {
                    self.play_sound(Sound::Invalid);
                    if let Some(challenge) = &mut self.challenge {
                        challenge.pending_removals = challenge.pending_removals.saturating_sub(1);
                    }
                }
                Event::Inspected(id, cell) => {
                    if self.selected_id == Some(id) {
                        self.gene_mode = cell.mode.max(0) as usize;
                        self.inspected = Some(cell);
                        self.edit_blueprint = false;
                        self.play_sound(Sound::Select);
                    }
                }
            }
        }
        self.preview.poll();
    }

    fn complete_challenge(&mut self) {
        let Some(challenge) = &mut self.challenge else {
            return;
        };
        if challenge.won {
            return;
        }
        challenge.won = true;
        let id = self.levels[challenge.index].id.clone();
        self.play_sound(Sound::Win);
        self.progress.completed.insert(id.clone());
        self.status = match self.progress.save() {
            Ok(()) => format!("Level {id} complete!"),
            Err(error) => format!("Level {id} complete, but progress could not be saved: {error}"),
        };
        self.send(Command::Play(false));
    }

    fn advance_tutorial(&mut self, direction: i8) {
        let mut finished = false;
        if let Some(challenge) = &mut self.challenge {
            if let Some(steps) = levels::tutorial_steps(&self.levels[challenge.index].id) {
                if direction < 0 {
                    challenge.tutorial_step = challenge.tutorial_step.saturating_sub(1);
                } else if direction > 0 && !challenge.won {
                    if challenge.tutorial_step + 1 < steps.len() {
                        challenge.tutorial_step += 1;
                    } else {
                        finished = true;
                    }
                }
            }
        }
        if finished {
            self.complete_challenge();
        } else if direction > 0 {
            self.play_sound(Sound::Tutorial);
        }
    }

    fn open_file(&mut self) {
        if self.open_path.trim().is_empty() {
            self.choose_open_file();
            return;
        }
        self.send(Command::Load(self.open_path.trim().to_owned()));
        if let Some(audio) = &mut self.audio {
            audio.set_track(Track::Experiment);
        }
        self.challenge = None;
        self.show_environment_settings = false;
        self.active_tool = Tool::Inspect;
        self.inspected = None;
        self.source_name = self.open_path.clone();
        self.status = "Opening plate…".into();
    }

    fn choose_open_file(&mut self) {
        match crate::file_dialog::open(self.open_path.trim()) {
            Ok(Some(path)) => {
                self.open_path = path.to_string_lossy().into_owned();
                self.open_file();
            }
            Ok(None) => {}
            Err(error) => self.status = error,
        }
    }

    fn save_file(&mut self) {
        if self.save_path.trim().is_empty() {
            self.choose_save_file();
            return;
        }
        self.send(Command::Save(self.save_path.trim().to_owned()));
        self.status = format!("Saving {}…", self.save_path);
    }

    fn choose_save_file(&mut self) {
        match crate::file_dialog::save(self.save_path.trim()) {
            Ok(Some(path)) => {
                self.save_path = path.to_string_lossy().into_owned();
                self.save_file();
            }
            Ok(None) => {}
            Err(error) => self.status = error,
        }
    }

    fn choose_save_genome(&mut self) {
        match crate::file_dialog::save_genome(&self.genome_path) {
            Ok(Some(path)) => {
                self.genome_path = path.to_string_lossy().into_owned();
                self.status = match legacy::export_genome(&self.blueprint_genes)
                    .and_then(|bytes| std::fs::write(&path, bytes).map_err(|e| e.to_string()))
                {
                    Ok(()) => format!("Genome saved to {}.", path.display()),
                    Err(error) => format!("Could not save genome: {error}"),
                };
                self.progress.blueprint = Some(self.blueprint_genes.clone());
                let _ = self.progress.save();
            }
            Ok(None) => {}
            Err(error) => self.status = error,
        }
    }

    fn choose_open_genome(&mut self) {
        match crate::file_dialog::open_genome(&self.genome_path) {
            Ok(Some(path)) => {
                self.genome_path = path.to_string_lossy().into_owned();
                self.status = match std::fs::read(&path)
                    .map_err(|e| e.to_string())
                    .and_then(|bytes| legacy::import_genome(&bytes))
                {
                    Ok(mut genes) => {
                        if let Some(challenge) = &self.challenge {
                            self.levels[challenge.index].adapt_genome(&mut genes);
                        }
                        self.blueprint_genes = genes;
                        self.blueprint_gene_mode = 0;
                        format!("Genome loaded from {}.", path.display())
                    }
                    Err(error) => format!("Could not load genome: {error}"),
                };
            }
            Ok(None) => {}
            Err(error) => self.status = error,
        }
    }

    fn genome_load_window(&mut self, ctx: &egui::Context) {
        if !self.show_genome_load {
            return;
        }
        let mut open = true;
        egui::Window::new("Load genome")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(250.0)
            .show(ctx, |ui| {
                if ui.button("Load from microscope").clicked() {
                    if let Some(cell) = &self.inspected {
                        self.blueprint_genes = cell.genes.clone();
                        self.blueprint_gene_mode = cell.mode.max(0) as usize;
                        self.status = "Selected organism's genome loaded.".into();
                    } else {
                        self.status = "Select an organism in the microscope first.".into();
                    }
                    self.show_genome_load = false;
                }
                if ui.button("Default genome").clicked() {
                    self.blueprint_genes = self
                        .challenge
                        .as_ref()
                        .map_or_else(default_player_genome, |challenge| {
                            self.levels[challenge.index].starting_genome()
                        });
                    self.blueprint_gene_mode = 0;
                    self.status = "Default genome loaded.".into();
                    self.show_genome_load = false;
                }
                if ui.button("From file…").clicked() {
                    self.choose_open_genome();
                    self.show_genome_load = false;
                }
            });
        self.show_genome_load &= open;
    }

    fn open_bundled(&mut self, index: usize) {
        self.send(Command::LoadBundled {
            index,
            as_challenge: false,
        });
        if let Some(audio) = &mut self.audio {
            audio.set_track(Track::Experiment);
        }
        self.challenge = None;
        self.show_environment_settings = false;
        self.active_tool = Tool::Inspect;
        self.inspected = None;
        self.source_name = bundled::PLATES[index].0.into();
        self.status = format!("Opening {}…", self.source_name);
    }

    fn open_challenge(&mut self, index: usize) {
        if let Some(plate_index) = bundled::PLATES
            .iter()
            .position(|(name, _)| *name == self.levels[index].plate)
        {
            self.show_environment_settings = false;
            self.levels[index].adapt_genome(&mut self.blueprint_genes);
            self.send(Command::LoadBundled {
                index: plate_index,
                as_challenge: true,
            });
            if let Some(audio) = &mut self.audio {
                audio.set_track(Track::Challenge);
            }
            self.source_name = format!("Level {}", self.levels[index].id);
            self.next_challenge_run_id += 1;
            let run_id = self.next_challenge_run_id;
            self.challenge = Some(Challenge {
                run_id,
                index,
                tutorial_step: 0,
                placed: 0,
                pending_places: 0,
                removed: 0,
                pending_removals: 0,
                won: false,
                pending: true,
            });
            if levels::tutorial_steps(&self.levels[index].id).is_none() {
                let level = &self.levels[index];
                self.send(Command::WatchChallenge(Some((
                    run_id,
                    level.conditions.clone(),
                    level.hold_time,
                ))));
            }
            self.active_tool = levels::TOOLS
                .into_iter()
                .find(|tool| self.levels[index].allows(*tool))
                .unwrap_or(Tool::Inspect);
            self.inspected = None;
            self.show_challenges = true;
            self.status = if levels::tutorial_steps(&self.levels[index].id).is_some() {
                format!(
                    "Tutorial {} loaded. Follow the guide on the left.",
                    self.levels[index].id
                )
            } else {
                format!(
                    "Level {} loaded. Place up to {} organisms.",
                    self.levels[index].id, self.levels[index].place_limit
                )
            };
        }
    }

    fn top_bar(&mut self, root: &mut egui::Ui) {
        egui::Panel::top("celllab-tabs")
            .exact_size(44.0)
            .show(root, |ui| {
                ui.horizontal_centered(|ui| {
                    ui.image((self.icons.plate[0].id(), egui::vec2(30.0, 30.0)));
                    ui.label(
                        egui::RichText::new(if self.challenge.is_some() {
                            "OBJECTIVE"
                        } else {
                            "SUBSTRATE"
                        })
                        .strong(),
                    );
                    ui.separator();
                    ui.label(
                        egui::RichText::new("MICROSCOPE")
                            .strong()
                            .color(Color32::from_rgb(255, 170, 0)),
                    );
                    ui.separator();
                    ui.label(egui::RichText::new("GENOME").strong());
                    ui.separator();
                    if ui
                        .add(egui::Button::image_and_text(
                            (self.icons.plate[1].id(), egui::vec2(22.0, 22.0)),
                            "Open",
                        ))
                        .clicked()
                    {
                        self.choose_open_file();
                    }
                    if ui
                        .add(egui::Button::image_and_text(
                            (self.icons.plate[2].id(), egui::vec2(22.0, 22.0)),
                            "Save",
                        ))
                        .clicked()
                    {
                        self.choose_save_file();
                    }
                    if ui
                        .add(egui::Button::image_and_text(
                            (self.icons.plate[3].id(), egui::vec2(22.0, 22.0)),
                            "Clear",
                        ))
                        .clicked()
                    {
                        self.send(Command::NewPlate);
                        self.play_sound(Sound::Button);
                        if let Some(audio) = &mut self.audio {
                            audio.set_track(Track::Experiment);
                        }
                        self.source_name = "new plate".into();
                        self.challenge = None;
                        self.show_environment_settings = false;
                        self.active_tool = Tool::Inspect;
                    }
                });
            });
    }

    fn plate_settings(&mut self, ui: &mut egui::Ui) {
        if self.challenge.is_some() {
            ui.label(
                egui::RichText::new(
                    "Challenge plate settings are fixed. Open this plate as a save to edit them.",
                )
                .small()
                .weak(),
            );
            return;
        }
        egui::CollapsingHeader::new("PLATE SETTINGS").default_open(true).show(ui, |ui| {
                ui.label("Plate radius");
                ui.horizontal(|ui| {
                    ui.add(egui::DragValue::new(&mut self.draft_radius).speed(0.01).range(0.05..=100.0));
                    if ui.button("Apply").clicked() {
                        self.send(Command::Radius(self.draft_radius));
                        self.status = "Plate radius updated. Organisms outside the boundary remain in the save until simulation resumes.".into();
                    }
                });
                ui.label("Maximum cells");
                ui.horizontal(|ui| {
                    ui.add(egui::DragValue::new(&mut self.draft_cells).range(0..=1_000_000));
                    if ui.button("Apply").clicked() { self.send(Command::MaxCells(self.draft_cells)); }
                });
                ui.label("Maximum food");
                ui.horizontal(|ui| {
                    ui.add(egui::DragValue::new(&mut self.draft_food_limit).range(0..=1_000_000));
                    if ui.button("Apply").clicked() { self.send(Command::MaxFood(self.draft_food_limit)); }
                });
                ui.label("Current food count");
                ui.horizontal(|ui| {
                    ui.add(egui::DragValue::new(&mut self.draft_food_count).range(0..=1_000_000));
                    if ui.button("Apply").clicked() { self.send(Command::FoodCount(self.draft_food_count)); }
                });
                ui.label(egui::RichText::new("Lowering a limit keeps organisms and food already in the save.").small().weak());
                if ui.button("Environment settings…").clicked() {
                    self.show_environment_settings = true;
                }
        });
    }

    fn environment_settings_window(&mut self, ctx: &egui::Context) {
        if !self.show_environment_settings || self.challenge.is_some() {
            return;
        }
        let more_parameters = self.unlocked_features()[0];
        let mut open = self.show_environment_settings;
        let mut apply = false;
        egui::Window::new("SUBSTRATE · Environment")
            .open(&mut open)
            .default_size([470.0, 590.0])
            .default_pos([315.0, 75.0])
            .resizable(true)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical()
                    .max_height((ui.available_height() - 38.0).max(200.0))
                    .show(ui, |ui| {
                        let env = &mut self.draft_environment;
                        ui.label("These settings affect this plate and are saved with it.");
                        ui.separator();
                        ui.heading("Food and nutrients");
                        ui.add(egui::Slider::new(&mut env.a, 0.0..=15.0).text("Nutrient rate"));
                        ui.add(
                            egui::Slider::new(&mut env.b, 0.0216..=1.152)
                                .text("Nutrient chunk size"),
                        );
                        if more_parameters {
                            ui.add(
                                egui::Slider::new(&mut env.o, 0.0..=1.0).text("Nutrient coating"),
                            );
                            ui.add(
                                egui::Slider::new(&mut env.p, 0.0..=100.0)
                                    .text("Nutrient lumpiness"),
                            );
                            ui.add(
                                egui::Slider::new(&mut env.q, 0.01..=0.2)
                                    .text("Nutrient lump size"),
                            );
                            ui.add(egui::Slider::new(&mut env.z, 0.0..=1.0).text("Nitrates"));
                            ui.add(egui::Slider::new(&mut env.n, 0.0..=1.0).text("Salinity"));
                        }
                        ui.checkbox(&mut env.y, "Mobile food");
                        ui.separator();
                        ui.heading("Light and gravity");
                        ui.add(egui::Slider::new(&mut env.d, 0.0..=2.0).text("Light amount"));
                        ui.add(egui::Slider::new(&mut env.f, 0.0..=1.0).text("Light range"));
                        if more_parameters {
                            ui.add(
                                egui::Slider::new(&mut env.e, 0.0..=1.0)
                                    .text("Light direction change"),
                            );
                        }
                        ui.add(egui::Slider::new(&mut env.h, -3.3..=3.3).text("Density"));
                        ui.add(egui::Slider::new(&mut env.i, 0.0..=1.0).text("Gravity"));
                        if more_parameters {
                            ui.add(
                                egui::Slider::new(&mut env.j, -2.0..=0.0).text("Density gradient"),
                            );
                        }
                        ui.separator();
                        ui.heading("Movement and evolution");
                        ui.add(
                            egui::Slider::new(&mut env.g, 0.0..=150.0).text("Substrate viscosity"),
                        );
                        if more_parameters {
                            ui.add(
                                egui::Slider::new(&mut env.m, 0.0..=8.0).text("Dynamic friction"),
                            );
                            ui.add(
                                egui::Slider::new(&mut env.l, 0.0..=8.0).text("Static friction"),
                            );
                        }
                        ui.add(egui::Slider::new(&mut env.c, 0.0..=0.09).text("Radiation level"));
                        ui.checkbox(&mut env.u, "Only point mutations");
                        ui.checkbox(&mut env.v, "Cell aging");
                        ui.checkbox(&mut env.w, "Contaminate with random cells");
                        ui.checkbox(&mut env.x, "Kill cells at edge");
                        ui.separator();
                        ui.collapsing("Cell types available to random seeding", |ui| {
                            env.enabled_types.resize(18, false);
                            if !env.enabled_types.iter().any(|enabled| *enabled) {
                                env.enabled_types[0] = true;
                            }
                            for kind in 0..18 {
                                let enabled_count =
                                    env.enabled_types.iter().filter(|enabled| **enabled).count();
                                ui.add_enabled(
                                    !env.enabled_types[kind] || enabled_count > 1,
                                    egui::Checkbox::new(
                                        &mut env.enabled_types[kind],
                                        cell_type_name(kind as i32),
                                    ),
                                );
                            }
                        });
                        if !more_parameters {
                            ui.label("More substrate parameters unlock after level 11.");
                        }
                    });
                ui.separator();
                if ui.button("Apply to plate").clicked() {
                    apply = true;
                }
            });
        self.show_environment_settings = open;
        if apply {
            self.send(Command::EnvironmentSettings(self.draft_environment.clone()));
            self.status = "Environment settings applied to this plate.".into();
        }
    }

    fn left_panel(&mut self, root: &mut egui::Ui) {
        egui::Panel::left("library")
            .min_size(220.0)
            .default_size(248.0)
            .show(root, |ui| {
                ui.add_space(15.0);
                ui.heading(if self.challenge.is_some() {
                    "OBJECTIVE"
                } else {
                    "SUBSTRATE"
                });
                ui.label(
                    egui::RichText::new("Windows port · working build")
                        .small()
                        .weak(),
                );
                let mut tutorial_navigation = 0;
                if let Some(challenge) = &self.challenge {
                    let level = &self.levels[challenge.index];
                    ui.separator();
                    ui.label(format!(
                        "Level {} · Difficulty {}",
                        level.id, level.difficulty
                    ));
                    ui.label(egui::RichText::new(&level.title).strong());
                    if let Some(steps) = levels::tutorial_steps(&level.id) {
                        let step_index = challenge.tutorial_step.min(steps.len() - 1);
                        let step = &steps[step_index];
                        ui.label(format!("Step {} of {}", step_index + 1, steps.len()));
                        ui.colored_label(Color32::from_rgb(255, 170, 0), step.title);
                        egui::ScrollArea::vertical()
                            .max_height(165.0)
                            .show(ui, |ui| {
                                ui.label(step.text);
                            });
                        ui.horizontal(|ui| {
                            if ui
                                .add_enabled(step_index > 0, egui::Button::new("Back"))
                                .clicked()
                            {
                                tutorial_navigation = -1;
                            }
                            let next_label = if step_index + 1 == steps.len() {
                                "Finish tutorial"
                            } else {
                                "Next"
                            };
                            if ui
                                .add_enabled(!challenge.won, egui::Button::new(next_label))
                                .clicked()
                            {
                                tutorial_navigation = 1;
                            }
                        });
                    } else {
                        egui::ScrollArea::vertical()
                            .max_height(130.0)
                            .show(ui, |ui| {
                                ui.label(&level.description);
                            });
                        if !level.hint.is_empty() {
                            ui.collapsing("Hint", |ui| {
                                ui.label(&level.hint);
                            });
                        }
                        if !level.tip.is_empty() {
                            ui.collapsing("Tip", |ui| {
                                ui.label(&level.tip);
                            });
                        }
                        ui.label(format!("Placed {}/{}", challenge.placed, level.place_limit));
                        if let Some(limit) = level.remove_limit {
                            ui.label(format!("Removed {}/{limit}", challenge.removed));
                        }
                        for condition in &level.conditions {
                            let met = levels::condition_met(condition, &self.snapshot);
                            ui.colored_label(
                                if met {
                                    Color32::LIGHT_GREEN
                                } else {
                                    Color32::LIGHT_GRAY
                                },
                                format!(
                                    "{} {}",
                                    levels::condition_label(&condition.tag),
                                    condition.target
                                ),
                            );
                        }
                        if level.hold_time > 0.0 {
                            ui.label(format!("Hold goals for {:.0} h", level.hold_time));
                        }
                    }
                    if challenge.won {
                        ui.colored_label(Color32::LIGHT_GREEN, "COMPLETED");
                    }
                }
                if tutorial_navigation != 0 {
                    self.advance_tutorial(tutorial_navigation);
                }
                ui.separator();
                ui.label("Open a save");
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut self.open_path).desired_width(165.0));
                    if ui.button("Browse").clicked() {
                        self.choose_open_file();
                    }
                });
                if ui.button("Open .substrate / .celllab2").clicked() {
                    self.open_file();
                }
                ui.add_space(10.0);
                ui.label("Save this plate");
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut self.save_path).desired_width(165.0));
                    if ui.button("Browse").clicked() {
                        self.choose_save_file();
                    }
                });
                if ui.button("Save copy (.celllab2 / .substrate)").clicked() {
                    self.save_file();
                }
                ui.separator();
                self.plate_settings(ui);
                egui::CollapsingHeader::new("AUDIO").show(ui, |ui| {
                    let mut music = self.progress.music_enabled;
                    if ui.checkbox(&mut music, "Music").changed() {
                        self.progress.music_enabled = music;
                        if let Some(audio) = &mut self.audio {
                            audio.set_music_enabled(music);
                        }
                        if let Err(error) = self.progress.save() {
                            self.status = format!("Audio preference could not be saved: {error}");
                        }
                    }
                    let mut effects = self.progress.effects_enabled;
                    if ui.checkbox(&mut effects, "Sound effects").changed() {
                        self.progress.effects_enabled = effects;
                        if let Some(audio) = &mut self.audio {
                            audio.set_effects_enabled(effects);
                        }
                        if let Err(error) = self.progress.save() {
                            self.status = format!("Audio preference could not be saved: {error}");
                        }
                    }
                });
                egui::CollapsingHeader::new("CREDITS").show(ui, |ui| {
                    ui.label("Cell Lab was created by Petter Säterskog.");
                    ui.label("Music and sound were created by Danilov Sound.");
                    ui.label(
                        "Challenges were designed by Alast, bwisialo, Nayus, and Petter Säterskog.",
                    );
                });
                ui.separator();
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut self.show_challenges, false, "Plates");
                    ui.selectable_value(&mut self.show_challenges, true, "Challenges");
                });
                ui.add(egui::TextEdit::singleline(&mut self.filter).hint_text("Filter plates"));
                let mut chosen_plate = None;
                let mut chosen_challenge = None;
                egui::ScrollArea::vertical().show(ui, |ui| {
                    if self.show_challenges {
                        ui.label(format!(
                            "{} / {} completed",
                            self.progress.completed.len(),
                            self.levels.len()
                        ));
                        for (index, level) in self.levels.iter().enumerate() {
                            if !level
                                .id
                                .to_lowercase()
                                .contains(&self.filter.to_lowercase())
                                && !level
                                    .title
                                    .to_lowercase()
                                    .contains(&self.filter.to_lowercase())
                            {
                                continue;
                            }
                            let selected =
                                self.challenge.as_ref().is_some_and(|c| c.index == index);
                            let completion = if self.progress.completed.contains(&level.id) {
                                " [complete]"
                            } else {
                                ""
                            };
                            let label = format!("{} · {}{}", level.id, level.title, completion);
                            if ui.selectable_label(selected, label).clicked() {
                                chosen_challenge = Some(index);
                            }
                        }
                    } else {
                        for (index, (name, _)) in bundled::PLATES.iter().enumerate() {
                            if !name.contains(&self.filter.to_lowercase()) {
                                continue;
                            }
                            if ui
                                .selectable_label(self.source_name == *name, name.replace('_', " "))
                                .clicked()
                            {
                                chosen_plate = Some(index);
                            }
                        }
                    }
                });
                if let Some(index) = chosen_plate {
                    self.open_bundled(index);
                }
                if let Some(index) = chosen_challenge {
                    self.open_challenge(index);
                }
            });
    }

    fn right_panel(&mut self, root: &mut egui::Ui) {
        let features = self.unlocked_features();
        egui::Panel::right("controls")
            .min_size(300.0)
            .default_size(320.0)
            .show(root, |ui| {
                ui.visuals_mut().override_text_color = Some(Color32::from_rgb(210, 210, 210));
                    ui.add_space(15.0);
                    ui.heading("GENOME");
                    ui.label(format!(
                        "{} organisms  ·  {} food",
                        self.snapshot.cells.len(),
                        self.snapshot.food.len()
                    ));
                    ui.label(format!("Simulation time {:.1} h", self.snapshot.time));
                    ui.label(egui::RichText::new(&self.status).small().weak());
                    ui.separator();
                    ui.label(egui::RichText::new("INSPECTOR").strong());
                    if let Some(index) = self.selected {
                        if let Some(cell) = self.snapshot.cells.get(index) {
                            ui.label(format!("Organism #{}", index + 1));
                            ui.label(format!(
                                "M{} · {}",
                                cell.mode.max(0) + 1,
                                cell_type_name(cell.cell_type)
                            ));
                            ui.label(format!("Tag: {}", cell_origin_name(cell.origin)));
                            ui.label(format!("Position: {:.3}, {:.3}", cell.x, cell.y));
                            ui.label(format!(
                                "Age: {:.2} h · Mass: {:.2} ng",
                                cell.age,
                                cell.mass * 10.0
                            ));
                            ui.label(format!(
                                "Diameter: {:.1} µm · Nitrogen: {:.0}%",
                                cell.radius * 1_000.0,
                                cell.nitrogen * 100.0
                            ));
                            ui.label(format!("Toxins: {:.2}", cell.toxins));
                            if cell.injury != 0.0 {
                                ui.label(format!("Injury: {:.0}%", cell.injury * 100.0));
                            }
                            if cell.lift != 0.0 {
                                ui.label(format!("Lift: {:.2}", cell.lift));
                            }
                            ui.label(format!(
                                "Mirrored: {} · Mutations: {}",
                                if cell.mirrored { "Yes" } else { "No" },
                                cell.mutations
                            ));
                            if cell.telomeres >= 0 {
                                ui.label(format!("Telomeres: {}", cell.telomeres));
                            }
                            if features[2] {
                                ui.label(format!(
                                    "Signals: {:.2}, {:.2}, {:.2}, {:.2}",
                                    cell.signal_levels[0].clamp(-1.0, 1.0),
                                    cell.signal_levels[1].clamp(-1.0, 1.0),
                                    cell.signal_levels[2].clamp(-1.0, 1.0),
                                    cell.signal_levels[3].clamp(-1.0, 1.0)
                                ));
                            }
                            if self.can_remove() && ui.button("Remove selected").clicked() {
                                self.send(Command::RemoveCell(cell.runtime_id));
                                self.selected = None;
                                self.selected_id = None;
                                self.inspected = None;
                                if let Some(challenge) = &mut self.challenge {
                                    challenge.pending_removals += 1;
                                }
                            }
                        }
                    } else {
                        ui.label("Click an organism to inspect it.");
                    }
                    let was_editing_blueprint = self.edit_blueprint;
                    ui.horizontal(|ui| {
                        ui.selectable_value(&mut self.edit_blueprint, true, "Synthesizer");
                        ui.selectable_value(&mut self.edit_blueprint, false, "Selected cell");
                    });
                    if self.edit_blueprint && !was_editing_blueprint {
                        self.play_sound(Sound::Deselect);
                    }
                    self.genome_preview(ui);
                    if self.edit_blueprint {
                        ui.separator();
                        let max_mode = self.blueprint_genes.len().saturating_sub(1);
                        let mut mode_action = None;
                        ui.horizontal(|ui| {
                            if ui.button("Save").clicked() {
                                self.choose_save_genome();
                            }
                            if ui.button("Load").clicked() {
                                self.show_genome_load = true;
                            }
                            ui.label("Edit mode");
                            mode_action = mode_combo(
                                ui,
                                "synthesizer_edit_mode",
                                &mut self.blueprint_gene_mode,
                                max_mode,
                                true,
                            );
                        });
                        if let Some(action) = mode_action {
                            let first_allowed_type = self.challenge.as_ref().and_then(|challenge| {
                                self.levels[challenge.index]
                                    .allowed_types
                                    .iter()
                                    .position(|allowed| *allowed)
                            }).unwrap_or(0) as i32;
                            apply_mode_action(
                                &mut self.blueprint_genes,
                                self.blueprint_gene_mode,
                                action,
                                first_allowed_type,
                            );
                        }
                        egui::ScrollArea::vertical()
                            .id_salt("synthesizer_gene_fields")
                            .max_height(ui.available_height())
                            .show(ui, |ui| {
                        let allowed_types = self.challenge.as_ref().map(|challenge| {
                            self.levels[challenge.index].allowed_types
                        });
                        let mut set_initial = false;
                        if let Some(gene) = self.blueprint_genes.get_mut(self.blueprint_gene_mode) {
                            genome_editor_frame(ui, |ui| {
                                gene_editor(
                                    ui,
                                    gene,
                                    max_mode,
                                    allowed_types.as_ref(),
                                    &mut self.oscillator,
                                    &mut self.snap_gene_sliders,
                                    features,
                                    &mut set_initial,
                                );
                            });
                        }
                        if set_initial {
                            for gene in &mut self.blueprint_genes {
                                gene.flags[4] = false;
                            }
                            self.blueprint_genes[self.blueprint_gene_mode].flags[4] = true;
                        }
                            });
                    } else if let Some(cell) = &mut self.inspected {
                        ui.separator();
                        ui.label(egui::RichText::new("GENOME").strong());
                        ui.label(format!("{} gene modes", cell.genes.len()));
                        let max_mode = cell.genes.len().saturating_sub(1);
                        let mut mode_action = None;
                        ui.horizontal(|ui| {
                            ui.label("Edit mode");
                            mode_action = mode_combo(ui, "selected_edit_mode", &mut self.gene_mode, max_mode, true);
                        });
                        if let Some(action) = mode_action {
                            let first_allowed_type = self.challenge.as_ref().and_then(|challenge| {
                                self.levels[challenge.index]
                                    .allowed_types
                                    .iter()
                                    .position(|allowed| *allowed)
                            }).unwrap_or(0) as i32;
                            apply_mode_action(&mut cell.genes, self.gene_mode, action, first_allowed_type);
                        }
                        egui::ScrollArea::vertical()
                            .id_salt("selected_gene_fields")
                            .max_height(ui.available_height())
                            .show(ui, |ui| {
                        let allowed_types = self.challenge.as_ref().map(|challenge| {
                            self.levels[challenge.index].allowed_types
                        });
                        let mut set_initial = false;
                        if let Some(gene) = cell.genes.get_mut(self.gene_mode) {
                            genome_editor_frame(ui, |ui| {
                                gene_editor(
                                    ui,
                                    gene,
                                    max_mode,
                                    allowed_types.as_ref(),
                                    &mut self.oscillator,
                                    &mut self.snap_gene_sliders,
                                    features,
                                    &mut set_initial,
                                );
                            });
                        }
                        if set_initial {
                            for gene in &mut cell.genes {
                                gene.flags[4] = false;
                            }
                            cell.genes[self.gene_mode].flags[4] = true;
                        }
                        let apply_genome = ui.button("Apply genome to selected organism").clicked();
                        let activate_mode =
                            ui.button("Apply and activate this gene mode").clicked();
                        if apply_genome || activate_mode {
                            if let Some(id) = self.selected_id {
                                let permitted = self.challenge.as_ref().is_none_or(|challenge| {
                                    self.levels[challenge.index].allows_genes(&cell.genes)
                                });
                                if permitted {
                                    let mode = if activate_mode {
                                        self.gene_mode as i32
                                    } else {
                                        cell.mode
                                    };
                                    let _ = self.engine.commands.send(Command::ReplaceGenome(
                                        id,
                                        cell.genes.clone(),
                                        mode,
                                    ));
                                    self.status = "Genome applied to selected organism.".into();
                                } else {
                                    self.status =
                                        "Genome uses a locked cell type or exceeds a challenge limit."
                                            .into();
                                }
                            }
                        }
                            });
                    }
            });
    }

    fn genome_preview(&mut self, ui: &mut egui::Ui) {
        let (genes, selected_mode) = if self.edit_blueprint {
            (&self.blueprint_genes, self.blueprint_gene_mode)
        } else {
            let Some(cell) = &self.inspected else { return };
            (&cell.genes, self.gene_mode)
        };
        if genes.is_empty() {
            return;
        }
        let time = if self.preview_progress <= 0.5 {
            self.preview_progress * 12.0
        } else {
            6.0 + (self.preview_progress - 0.5) * 84.0
        };
        self.preview.request(genes, time);
        ui.label(egui::RichText::new("CELL PREVIEW").strong());
        let (rect, _) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), 136.0),
            egui::Sense::hover(),
        );
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 4.0, Color32::from_rgb(32, 32, 32));
        if let Some(snapshot) = self.preview.snapshot() {
            let scale = rect.height() * 4.0;
            crate::apk_links::paint_preview(
                &painter,
                rect,
                &snapshot.links,
                rect.center(),
                scale,
                self.cell_format,
            );
            crate::apk_cells::paint_preview(
                &painter,
                rect,
                &snapshot.cells,
                rect.center(),
                scale,
                self.cell_format,
                selected_mode,
            );
        }
        ui.add(
            egui::Slider::new(&mut self.preview_progress, 0.0..=1.0)
                .text("Time")
                .show_value(false),
        );
        let (shown_time, count) = self
            .preview
            .snapshot()
            .map_or((0.0, 0), |snapshot| (snapshot.time, snapshot.cells.len()));
        ui.small(format!("{shown_time:.1} h simulated · {count} cells"));
    }

    fn microscope_toolbar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 3.0;
            ui.label(egui::RichText::new("MICROSCOPE").strong());
            for tool in levels::TOOLS {
                if self.tool_allowed(tool) {
                    let icon = &self.icons.tools[tool as usize];
                    if ui
                        .add(
                            egui::Button::image_and_text(
                                (icon.id(), egui::vec2(20.0, 20.0)),
                                tool.label(),
                            )
                            .selected(self.active_tool == tool),
                        )
                        .clicked()
                    {
                        self.active_tool = tool;
                    }
                }
            }
            if ui
                .button(if self.snapshot.running {
                    "Pause"
                } else {
                    "Play"
                })
                .clicked()
            {
                self.send(Command::Play(!self.snapshot.running));
            }
            if ui.button("Step").clicked() {
                self.send(Command::Step);
            }
            if ui
                .add(
                    egui::DragValue::new(&mut self.speed_input)
                        .speed(0.1)
                        .range(0.1..=f64::MAX)
                        .suffix("×"),
                )
                .on_hover_text("Enter any positive speed. Incubate runs as fast as this PC allows.")
                .changed()
            {
                self.send(Command::Speed(self.speed_input));
            }
            let incubating = self.snapshot.speed == f64::INFINITY;
            if ui
                .add(egui::Button::new("Incubate").selected(incubating))
                .clicked()
            {
                if incubating {
                    self.send(Command::Speed(self.speed_input));
                } else {
                    self.send(Command::Speed(f64::INFINITY));
                    self.send(Command::Play(true));
                }
            }
        });
        ui.separator();
    }

    fn plate(&mut self, ui: &mut egui::Ui) {
        self.microscope_toolbar(ui);
        if self.active_tool != Tool::Move {
            if let Some(id) = self.moving.take() {
                self.send(Command::ReleaseCell(id));
            }
        }
        let (rect, response) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
        let painter = ui.painter_at(rect);
        let clear = if !self.snapshot.running {
            Color32::from_rgb(217, 217, 255)
        } else if self.snapshot.speed >= 8.0 {
            Color32::from_rgb(230, 191, 166)
        } else if (self.snapshot.speed - 1.0).abs() < 0.05 {
            Color32::from_rgb(186, 179, 242)
        } else {
            Color32::from_rgb(217, 255, 217)
        };
        painter.rect_filled(rect, 0.0, clear);
        let center = rect.center() + self.pan;
        let scale = rect.width().min(rect.height()) * 0.45 * self.zoom
            / self.snapshot.radius.max(0.05) as f32;
        let plate_radius = self.snapshot.radius as f32 * scale;
        crate::apk_plate::paint(
            &painter,
            rect,
            center,
            plate_radius,
            self.snapshot.light_angle as f32,
            self.snapshot.light_amount as f32,
            self.snapshot.light_range as f32,
            self.cell_format,
        );
        let bounds = rect.expand(10.0);
        crate::apk_food::paint(
            &painter,
            rect,
            &self.snapshot.food,
            center,
            scale,
            self.cell_format,
        );
        crate::apk_links::paint(
            &painter,
            rect,
            &self.snapshot.links,
            center,
            scale,
            self.cell_format,
        );
        crate::apk_dead::paint(
            &painter,
            rect,
            &self.snapshot.dead,
            center,
            scale,
            self.cell_format,
        );
        crate::apk_cells::paint(
            &painter,
            rect,
            &self.snapshot.cells,
            center,
            scale,
            self.cell_format,
        );
        for (index, cell) in self.snapshot.cells.iter().enumerate() {
            let pos = center + Vec2::new(cell.x * scale, cell.y * scale);
            let size = (cell.radius * scale).max(1.0);
            if pos.x + size < bounds.left()
                || pos.x - size > bounds.right()
                || pos.y + size < bounds.top()
                || pos.y - size > bounds.bottom()
            {
                continue;
            }
            if self.selected == Some(index) {
                painter.circle_stroke(
                    pos,
                    size + 4.0,
                    Stroke::new(2.0, Color32::from_rgb(255, 170, 0)),
                );
            }
        }
        let nearest_cell = |pointer: Pos2| {
            self.snapshot
                .cells
                .iter()
                .enumerate()
                .filter_map(|(i, cell)| {
                    let position = center + Vec2::new(cell.x * scale, cell.y * scale);
                    let distance = position.distance(pointer);
                    (distance < (cell.radius * scale).max(8.0)).then_some((i, distance))
                })
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(i, _)| i)
        };
        if response.drag_started() && self.active_tool == Tool::Move {
            self.moving = response
                .interact_pointer_pos()
                .and_then(nearest_cell)
                .map(|index| self.snapshot.cells[index].runtime_id);
            if let Some(id) = self.moving {
                self.send(Command::GrabCell(id));
            }
        }
        if response.dragged() {
            if let (Some(id), Some(pointer)) = (self.moving, response.interact_pointer_pos()) {
                let world = (pointer - center) / scale;
                self.send(Command::MoveCell(id, world.x as f64, world.y as f64));
            } else {
                self.pan += response.drag_delta();
            }
        }
        if response.drag_stopped() {
            if let Some(id) = self.moving.take() {
                self.send(Command::ReleaseCell(id));
            }
        }
        if response.hovered() {
            let zoom_delta = ui.input(|i| i.smooth_scroll_delta.y);
            if zoom_delta.abs() > 0.1 {
                if let Some(pointer) = response.hover_pos() {
                    zoom_at_pointer(
                        &mut self.zoom,
                        &mut self.pan,
                        rect.center(),
                        pointer,
                        zoom_delta,
                    );
                }
            }
        }
        if response.clicked() {
            if let Some(pointer) = response.interact_pointer_pos() {
                let world = (pointer - center) / scale;
                let tool = if ui.input(|i| i.modifiers.shift) && self.tool_allowed(Tool::Cell) {
                    Tool::Cell
                } else {
                    self.active_tool
                };
                match tool {
                    Tool::Cell => {
                        let clone_selected =
                            ui.input(|i| i.modifiers.shift) && self.selected.is_some();
                        let existing = if clone_selected {
                            None
                        } else {
                            nearest_cell(pointer)
                        };
                        if let Some(index) = existing {
                            self.selected = Some(index);
                            let id = self.snapshot.cells[index].runtime_id;
                            self.selected_id = Some(id);
                            self.inspected = None;
                            self.send(Command::InspectCell(id));
                        } else {
                            let allowed = self.challenge.as_ref().is_none_or(|c| {
                                c.placed + c.pending_places < self.levels[c.index].place_limit
                            });
                            let genome_allowed = self.challenge.as_ref().is_none_or(|c| {
                                clone_selected
                                    || self.levels[c.index].allows_genes(&self.blueprint_genes)
                            });
                            if allowed
                                && genome_allowed
                                && self.snapshot.max_cells > 0
                                && self.snapshot.cells.len() <= self.snapshot.max_cells
                                && world.length() <= self.snapshot.radius as f32
                            {
                                self.send(Command::AddCell(
                                    world.x as f64,
                                    world.y as f64,
                                    if clone_selected {
                                        self.selected_id
                                    } else {
                                        None
                                    },
                                    if clone_selected {
                                        None
                                    } else {
                                        Some(self.blueprint_genes.clone())
                                    },
                                    self.challenge
                                        .as_ref()
                                        .map(|c| self.levels[c.index].gene_limits()),
                                ));
                                if let Some(challenge) = &mut self.challenge {
                                    challenge.pending_places += 1;
                                }
                            } else {
                                self.status =
                            "Cannot place this organism: check plate, population, genome, and challenge limits.".into();
                            }
                        }
                    }
                    Tool::Boost => {
                        if let Some(index) = nearest_cell(pointer) {
                            self.send(Command::BoostCell(self.snapshot.cells[index].runtime_id));
                        } else if world.length() <= self.snapshot.radius as f32 {
                            self.send(Command::AddFood(world.x as f64, world.y as f64));
                        }
                    }
                    Tool::Remove => {
                        if let Some(index) = nearest_cell(pointer) {
                            if self.can_remove() {
                                self.send(Command::RemoveCell(
                                    self.snapshot.cells[index].runtime_id,
                                ));
                                if let Some(challenge) = &mut self.challenge {
                                    challenge.pending_removals += 1;
                                }
                                self.selected = None;
                                self.selected_id = None;
                                self.inspected = None;
                            } else {
                                self.status = "Removal limit reached for this challenge.".into();
                            }
                        }
                    }
                    Tool::Inspect => {
                        self.selected = nearest_cell(pointer);
                        self.selected_id = self
                            .selected
                            .map(|index| self.snapshot.cells[index].runtime_id);
                        self.inspected = None;
                        if let Some(id) = self.selected_id {
                            self.send(Command::Play(false));
                            self.send(Command::InspectCell(id));
                        }
                    }
                    Tool::Move => {}
                }
            }
        }
        painter.text(
            Pos2::new(rect.left() + 17.0, rect.bottom() - 28.0),
            egui::Align2::LEFT_CENTER,
            format!("{}  ·  {:.0}% zoom", self.source_name, self.zoom * 100.0),
            egui::FontId::proportional(13.0),
            Color32::from_rgb(32, 32, 32),
        );
    }
}

impl eframe::App for App {
    fn ui(&mut self, root: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.poll();
        let files = root.input(|i| i.raw.dropped_files.clone());
        if let Some(path) = files.first().map(|f| f.path()) {
            self.open_path = path.to_string_lossy().into_owned();
            self.open_file();
        }
        self.top_bar(root);
        self.left_panel(root);
        self.right_panel(root);
        egui::CentralPanel::default().show(root, |ui| self.plate(ui));
        self.environment_settings_window(root.ctx());
        self.genome_load_window(root.ctx());
        if !self.capture_requested
            && self.preview.current_frame_ready()
            && self
                .challenge
                .as_ref()
                .is_none_or(|challenge| !challenge.pending)
        {
            if let Some(path) = self.capture_path.take() {
                self.capture_requested = true;
                let ctx = root.ctx().clone();
                root.ctx().request_screenshot(move |image| {
                    if let Err(error) = crate::visuals::save_png(&path, &image) {
                        eprintln!("UI capture failed: {error}");
                    }
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                });
            }
        }
        root.ctx().request_repaint_after(Duration::from_millis(16));
    }
}

fn genome_editor_frame(ui: &mut egui::Ui, content: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .fill(Color32::from_rgb(48, 48, 48))
        .stroke(Stroke::new(1.0, Color32::from_rgb(56, 129, 11)))
        .inner_margin(6)
        .show(ui, content);
}

#[derive(Clone, Copy)]
enum ModeAction {
    Reset(usize),
    Copy(usize),
}

fn apply_mode_action(genes: &mut [Gene], source: usize, action: ModeAction, first_type: i32) {
    let target = match action {
        ModeAction::Reset(target) | ModeAction::Copy(target) => target,
    };
    let Some(existing) = genes.get(target) else {
        return;
    };
    let initial = existing.flags[4];
    let mut replacement = match action {
        ModeAction::Reset(_) => default_gene_for_mode(target, first_type),
        ModeAction::Copy(_) => match genes.get(source) {
            Some(gene) => gene.clone(),
            None => return,
        },
    };
    // The APK preserves a destination mode's Initial flag when copying.
    // Keep one initial mode when resetting a slot as well.
    replacement.flags[4] = initial;
    genes[target] = replacement;
}

fn mode_combo(
    ui: &mut egui::Ui,
    id: &str,
    mode: &mut usize,
    max_mode: usize,
    edit_actions: bool,
) -> Option<ModeAction> {
    *mode = (*mode).min(max_mode);
    let mut action = None;
    egui::ComboBox::from_id_salt(id)
        .width(62.0)
        .selected_text(format!("M{}", *mode + 1))
        .show_ui(ui, |ui| {
            for index in 0..=max_mode {
                let response = ui.selectable_value(mode, index, format!("M{}", index + 1));
                if edit_actions {
                    response
                        .on_hover_text("Right-click to reset or copy this mode")
                        .context_menu(|ui| {
                            if ui
                                .button(format!("Load default to M{}", index + 1))
                                .clicked()
                            {
                                action = Some(ModeAction::Reset(index));
                                ui.close();
                            }
                            if ui
                                .button(format!("Copy M{} to M{}", *mode + 1, index + 1))
                                .clicked()
                            {
                                action = Some(ModeAction::Copy(index));
                                ui.close();
                            }
                        });
                }
            }
        });
    action
}

fn mode_reference_combo(ui: &mut egui::Ui, label: &str, mode: &mut i32, max_mode: usize) {
    ui.horizontal(|ui| {
        ui.label(label);
        let mut selected = (*mode).clamp(0, max_mode as i32) as usize;
        let _ = mode_combo(ui, label, &mut selected, max_mode, false);
        *mode = selected as i32;
    });
}

fn choice_combo(ui: &mut egui::Ui, label: &str, value: &mut i32, choices: &[&str]) {
    ui.horizontal(|ui| {
        ui.label(label);
        let selected = choices.get(*value as usize).copied().unwrap_or("?");
        egui::ComboBox::from_id_salt(label)
            .selected_text(selected)
            .show_ui(ui, |ui| {
                for (index, name) in choices.iter().enumerate() {
                    ui.selectable_value(value, index as i32, *name);
                }
            });
    });
}

fn numbered_combo(ui: &mut egui::Ui, label: &str, value: &mut i32, count: i32) {
    ui.horizontal(|ui| {
        ui.label(label);
        egui::ComboBox::from_id_salt(label)
            .selected_text(format!("{}", *value + 1))
            .show_ui(ui, |ui| {
                for index in 0..count {
                    ui.selectable_value(value, index, format!("{}", index + 1));
                }
            });
    });
}

fn add_gene_slider(
    ui: &mut egui::Ui,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
    label: &str,
    snap: bool,
    enabled: bool,
) -> egui::Response {
    let step_count = if snap { 24.0 } else { 1200.0 };
    let step = (*range.end() - *range.start()) as f64 / step_count;
    ui.add_enabled(
        enabled,
        egui::Slider::new(value, range).text(label).step_by(step),
    )
}

fn priority_from_fraction(fraction: f32) -> f32 {
    0.1 * 100.0_f32.powf(fraction.clamp(0.0, 1.0))
}

fn priority_fraction(priority: f32) -> f32 {
    (priority.clamp(0.1, 10.0) / 0.1).ln() / 100.0_f32.ln()
}

fn apk_action_bias(progress: i32, ticks: i32, positive: bool) -> f32 {
    const FIRST: f64 = 0.08246923983097076;
    const CURVE: f64 = 97.00586700439453;
    if positive {
        if progress == 0 {
            return -1.0;
        }
        let signal = FIRST * CURVE.powf((progress - 1) as f64 / (ticks - 1) as f64);
        return (signal / 4.0 - 1.0) as f32;
    }
    let middle = ticks / 2;
    if progress == middle {
        return 0.0;
    }
    let distance = (progress - middle).abs();
    let signal = FIRST * CURVE.powf((distance - 1) as f64 / (middle - 1) as f64);
    (signal * (progress - middle).signum() as f64 / 8.0) as f32
}

fn apk_action_progress(bias: f32, ticks: i32, positive: bool) -> i32 {
    const FIRST: f64 = 0.08246923983097076;
    const CURVE: f64 = 97.00586700439453;
    let bias = bias.clamp(-1.0, 1.0) as f64;
    if positive {
        let signal = (bias + 1.0) * 4.0;
        if signal <= 0.0 {
            return 0;
        }
        let progress = 1.0 + (signal / FIRST).ln() / CURVE.ln() * (ticks - 1) as f64;
        return (progress.round() as i32).clamp(0, ticks);
    }
    if bias == 0.0 {
        return ticks / 2;
    }
    let middle = ticks / 2;
    let signal = bias.abs() * 8.0;
    let distance = 1.0 + (signal / FIRST).ln() / CURVE.ln() * (middle - 1) as f64;
    (middle + bias.signum() as i32 * distance.round() as i32).clamp(0, ticks)
}

fn gene_editor(
    ui: &mut egui::Ui,
    gene: &mut Gene,
    max_mode: usize,
    allowed_types: Option<&[bool; 18]>,
    oscillator: &mut OscillatorControls,
    snap: &mut bool,
    features: [bool; 3],
    set_initial: &mut bool,
) {
    let program_options_enabled =
        allowed_types.is_none_or(|types| types[10] || types[11] || types[13]);
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.checkbox(&mut gene.flags[0], "Make adhesin");
            if features[1] {
                ui.checkbox(&mut gene.flags[3], "Prioritize");
            }
        });
        ui.vertical(|ui| {
            ui.horizontal(|ui| {
                ui.label("Cell type");
                egui::ComboBox::from_id_salt("gene_cell_type")
                    .selected_text(cell_type_name(gene.cell_type))
                    .show_ui(ui, |ui| {
                        for kind in 0..18 {
                            if allowed_types.is_none_or(|types| types[kind as usize]) {
                                ui.selectable_value(
                                    &mut gene.cell_type,
                                    kind,
                                    cell_type_name(kind),
                                );
                            }
                        }
                    });
            });
            if ui.radio(gene.flags[4], "Initial").clicked() {
                *set_initial = true;
            }
        });
    });
    ui.separator();
    ui.strong("CHILD 1");
    ui.horizontal_wrapped(|ui| {
        ui.label("Mode");
        let mut mode = gene.i.clamp(0, max_mode as i32) as usize;
        let _ = mode_combo(ui, "child_1_mode", &mut mode, max_mode, false);
        gene.i = mode as i32;
        ui.checkbox(&mut gene.flags[1], "Keep adhesin");
        if features[1] {
            ui.checkbox(&mut gene.flags[5], "Mirror");
        }
    });
    ui.separator();
    ui.strong("CHILD 2");
    ui.horizontal_wrapped(|ui| {
        ui.label("Mode");
        let mut mode = gene.j.clamp(0, max_mode as i32) as usize;
        let _ = mode_combo(ui, "child_2_mode", &mut mode, max_mode, false);
        gene.j = mode as i32;
        ui.checkbox(&mut gene.flags[2], "Keep adhesin");
        if features[1] {
            ui.checkbox(&mut gene.flags[6], "Mirror");
        }
    });
    ui.checkbox(snap, "Snap");
    ui.separator();
    add_gene_slider(
        ui,
        &mut gene.floats[0],
        0.0648..=0.3672,
        "Split mass",
        *snap,
        true,
    );
    if features[1] {
        let mut child_one_share = 1.0 - gene.floats[1];
        if add_gene_slider(
            ui,
            &mut child_one_share,
            0.1..=0.9,
            "Split ratio",
            *snap,
            true,
        )
        .changed()
        {
            gene.floats[1] = 1.0 - child_one_share;
        }
        let mut priority_position = priority_fraction(gene.floats[5]);
        let ticks = if *snap { 24.0 } else { 1200.0 };
        let slider = egui::Slider::new(&mut priority_position, 0.0..=1.0)
            .text("Nutrient priority")
            .step_by(1.0 / ticks)
            .custom_formatter(|value, _| format!("{:.2}", priority_from_fraction(value as f32)))
            .custom_parser(|text| {
                text.parse::<f32>()
                    .ok()
                    .filter(|value| value.is_finite() && *value > 0.0)
                    .map(priority_fraction)
                    .map(f64::from)
            });
        if ui.add(slider).changed() {
            gene.floats[5] = priority_from_fraction(priority_position);
        }
    }
    add_gene_slider(
        ui,
        &mut gene.floats[2],
        0.0..=std::f32::consts::TAU,
        "Split angle",
        *snap,
        true,
    );
    add_gene_slider(
        ui,
        &mut gene.floats[3],
        0.0..=std::f32::consts::TAU,
        "Child 1 angle",
        *snap,
        true,
    );
    add_gene_slider(
        ui,
        &mut gene.floats[4],
        0.0..=std::f32::consts::TAU,
        "Child 2 angle",
        *snap,
        true,
    );
    add_gene_slider(ui, &mut gene.color[0], 0.0..=1.0, "Red color", *snap, true);
    add_gene_slider(
        ui,
        &mut gene.color[1],
        0.0..=1.0,
        "Green color",
        *snap,
        true,
    );
    add_gene_slider(ui, &mut gene.color[2], 0.0..=1.0, "Blue color", *snap, true);
    if features[1] {
        add_gene_slider(
            ui,
            &mut gene.floats[6],
            0.0..=35.0,
            "Adhesin stiffness",
            *snap,
            gene.flags[0],
        );
    }
    if !features[1] {
        ui.label("More genome parameters unlock after level 16.");
    }
    ui.separator();
    add_gene_slider(
        ui,
        &mut gene.output_values[6],
        0.5..=2.0,
        "Cytoskeleton",
        *snap,
        true,
    );
    add_gene_slider(
        ui,
        &mut gene.output_values[5],
        0.0..=0.01,
        "Adhesin length",
        *snap,
        gene.flags[0],
    );
    if features[1] {
        numbered_combo(ui, "Maximum connections", &mut gene.output_modes[9], 20);
    }
    ui.separator();
    match gene.cell_type {
        8 => {
            mode_reference_combo(ui, "Virus copy from", &mut gene.output_modes[0], max_mode);
        }
        10 | 11 => {
            choice_combo(
                ui,
                "Smell type",
                &mut gene.output_modes[3],
                &["Cell", "Food", "Coated food", "Light", "Velocity", "Wall"],
            );
            choice_combo(
                ui,
                "Output channel",
                &mut gene.output_modes[2],
                &["S1", "S2", "S3", "S4"],
            );
            for (index, name) in [
                "Output amount",
                "Smell red",
                "Smell green",
                "Smell blue",
                "Smell threshold",
            ]
            .iter()
            .enumerate()
            {
                let range = match index {
                    0 => -60.0..=60.0,
                    4 => 0.0..=1.732_050_8,
                    _ => 0.0..=1.0,
                };
                add_gene_slider(ui, &mut gene.output_values[index], range, name, *snap, true);
            }
        }
        14 => {
            choice_combo(
                ui,
                "Secretion",
                &mut gene.output_modes[4],
                &[
                    "Food smell",
                    "Cyanide",
                    "Lipase",
                    "Protease",
                    "Coated food",
                    "Light",
                    "Wall",
                    "-S1",
                    "-S2",
                    "-S3",
                    "-S4",
                    "+S1",
                    "+S2",
                    "+S3",
                    "+S4",
                ],
            );
        }
        15 => {
            mode_reference_combo(ui, "First switch mode", &mut gene.output_modes[0], max_mode);
            mode_reference_combo(
                ui,
                "Second switch mode",
                &mut gene.output_modes[1],
                max_mode,
            );
        }
        16 => {
            mode_reference_combo(ui, "Compatible mode", &mut gene.output_modes[1], max_mode);
            mode_reference_combo(
                ui,
                "Mode after fertilization",
                &mut gene.output_modes[0],
                max_mode,
            );
            choice_combo(
                ui,
                "Telomeres",
                &mut gene.output_modes[10],
                &["∞", "5", "10", "20", "40", "80"],
            );
        }
        13 => {
            for channel in 0..4 {
                choice_combo(
                    ui,
                    &format!("Signal {} channel", channel + 1),
                    &mut gene.output_modes[channel + 5],
                    &["S1", "S2", "S3", "S4"],
                );
            }
        }
        _ => {}
    }
    match gene.cell_type {
        1 => cell_action_slider(
            ui,
            gene,
            oscillator,
            *snap,
            program_options_enabled,
            0,
            "Swim force",
        ),
        6 => cell_action_slider(
            ui,
            gene,
            oscillator,
            *snap,
            program_options_enabled,
            1,
            "Density",
        ),
        12 => {
            cell_action_slider(
                ui,
                gene,
                oscillator,
                *snap,
                program_options_enabled,
                3,
                "Muscle contraction",
            );
            cell_action_slider(
                ui,
                gene,
                oscillator,
                *snap,
                program_options_enabled,
                2,
                "Muscle bend",
            );
            cell_action_slider(
                ui,
                gene,
                oscillator,
                *snap,
                program_options_enabled,
                4,
                "Muscle lift",
            );
        }
        13 => {
            for channel in 0..4 {
                cell_action_slider(
                    ui,
                    gene,
                    oscillator,
                    *snap,
                    program_options_enabled,
                    channel + 7,
                    &format!("Signal {} production", channel + 1),
                );
            }
        }
        15 => {
            cell_action_slider(
                ui,
                gene,
                oscillator,
                *snap,
                program_options_enabled,
                5,
                "First mode switch",
            );
            cell_action_slider(
                ui,
                gene,
                oscillator,
                *snap,
                program_options_enabled,
                6,
                "Second mode switch",
            );
        }
        17 => {
            cell_action_slider(
                ui,
                gene,
                oscillator,
                *snap,
                program_options_enabled,
                2,
                "Left cilia speed",
            );
            cell_action_slider(
                ui,
                gene,
                oscillator,
                *snap,
                program_options_enabled,
                4,
                "Right cilia speed",
            );
        }
        _ => {}
    };
    if program_options_enabled
        && gene.cell_type == 13
        && ui.button("Oscillator preset...").clicked()
    {
        oscillator.show_preset = true;
    }
    draw_gene_dialogs(ui.ctx(), gene, oscillator, *snap);
}

fn draw_gene_dialogs(
    ctx: &egui::Context,
    gene: &mut Gene,
    controls: &mut OscillatorControls,
    snap: bool,
) {
    if controls.show_preset {
        let mut open = true;
        egui::Window::new("Oscillator preset")
            .open(&mut open)
            .default_width(350.0)
            .show(ctx, |ui| {
                ui.label("Make two signal channels oscillate in a Neurocyte.");
                ui.add(
                    egui::Slider::new(&mut controls.connections, 0..=5).text("Adhesin connections"),
                );
                egui::ComboBox::from_label("First channel")
                    .selected_text(format!("S{}", controls.first_channel + 1))
                    .show_ui(ui, |ui| {
                        for channel in 0..4 {
                            ui.selectable_value(
                                &mut controls.first_channel,
                                channel,
                                format!("S{}", channel + 1),
                            );
                        }
                    });
                egui::ComboBox::from_label("Second channel")
                    .selected_text(format!("S{}", controls.second_channel + 1))
                    .show_ui(ui, |ui| {
                        for channel in 0..4 {
                            ui.selectable_value(
                                &mut controls.second_channel,
                                channel,
                                format!("S{}", channel + 1),
                            );
                        }
                    });
                add_gene_slider(
                    ui,
                    &mut controls.period,
                    0.25..=9.25,
                    "Period (hours)",
                    snap,
                    true,
                );
                if ui
                    .add_enabled(
                        controls.first_channel != controls.second_channel,
                        egui::Button::new("Apply preset"),
                    )
                    .clicked()
                {
                    let _ = oscillator::apply_preset(
                        gene,
                        controls.connections,
                        controls.first_channel,
                        controls.second_channel,
                        controls.period,
                    );
                }
            });
        controls.show_preset = open;
    }
    if let Some(mut output) = controls.programmed_output {
        const SOURCES: [&str; 8] = [
            "Signal 1",
            "Signal 2",
            "Signal 3",
            "Signal 4",
            "Mass",
            "Age",
            "Nitro reserve",
            "Links",
        ];
        const RANGES: [&str; 11] = [
            "0 to 1",
            "-3 to 3",
            "-1 to 1",
            "-0.5 to 0.5",
            "-1 to 1",
            "-1 to 1",
            "-1 to 1",
            "-20 to 20",
            "-20 to 20",
            "-20 to 20",
            "-20 to 20",
        ];
        let mut open = true;
        egui::Window::new("Program signal")
            .open(&mut open)
            .default_width(350.0)
            .show(ctx, |ui| {
                egui::ComboBox::from_label("Output")
                    .selected_text(format!("{} ({})", output + 1, RANGES[output]))
                    .show_ui(ui, |ui| {
                        for (index, range) in RANGES.iter().enumerate() {
                            ui.selectable_value(
                                &mut output,
                                index,
                                format!("{} ({range})", index + 1),
                            );
                        }
                    });
                let input = &mut gene.inputs[output];
                egui::ComboBox::from_label("Input source")
                    .selected_text(SOURCES[input.source.clamp(0, 7) as usize])
                    .show_ui(ui, |ui| {
                        for (source, label) in SOURCES.iter().enumerate() {
                            ui.selectable_value(&mut input.source, source as i16, *label);
                        }
                    });
                egui::ComboBox::from_label("Program")
                    .selected_text(match input.mode {
                        0 => "Constant",
                        1 => "Linear",
                        _ => "Threshold",
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut input.mode, 0, "Constant");
                        ui.selectable_value(&mut input.mode, 1, "Linear");
                        ui.selectable_value(&mut input.mode, 2, "Threshold");
                    });
                match input.mode {
                    0 => {
                        add_gene_slider(ui, &mut input.b, -1.0..=1.0, "Constant", snap, true);
                    }
                    1 => {
                        add_gene_slider(ui, &mut input.a, -8.0..=8.0, "Slope", snap, true);
                        add_gene_slider(ui, &mut input.b, -8.0..=8.0, "Bias", snap, true);
                    }
                    _ => {
                        add_gene_slider(ui, &mut input.c, -1.0..=1.0, "Threshold", snap, true);
                        add_gene_slider(ui, &mut input.a, -1.0..=1.0, "Below", snap, true);
                        add_gene_slider(ui, &mut input.b, -1.0..=1.0, "Above", snap, true);
                    }
                }
                ui.label("The result is clamped to -1..1, then mapped to this output's range.");
            });
        controls.programmed_output = open.then_some(output);
    }
}

fn cell_action_slider(
    ui: &mut egui::Ui,
    gene: &mut Gene,
    controls: &mut OscillatorControls,
    snap: bool,
    show_more: bool,
    output: usize,
    label: &str,
) {
    let Some(input) = gene.inputs.get_mut(output) else {
        return;
    };
    let Some((lower, upper)) = GENE_OUTPUT_RANGES.get(output).copied() else {
        return;
    };
    let ticks = if snap { 24 } else { 1200 };
    let positive = lower == 0.0;
    let mut progress = apk_action_progress(input.b, ticks, positive);
    let title = match input.mode {
        0 => label.to_owned(),
        1 => format!("{label} bias"),
        _ => format!("{label} above threshold"),
    };
    ui.horizontal(|ui| {
        if ui
            .add_enabled(
                input.mode == 0,
                egui::Slider::new(&mut progress, 0..=ticks)
                    .text(title)
                    .show_value(false),
            )
            .changed()
        {
            input.b = apk_action_bias(progress, ticks, positive);
        }
        let value = lower + (input.b.clamp(-1.0, 1.0) + 1.0) * (upper - lower) * 0.5;
        ui.label(format!("{value:.2}"));
        if show_more && ui.button("More...").clicked() {
            controls.programmed_output = Some(output);
        }
    });
}

fn cell_type_name(value: i32) -> &'static str {
    const NAMES: [&str; 18] = [
        "Phagocyte",
        "Flagellocyte",
        "Photocyte",
        "Devorocyte",
        "Lipocyte",
        "Keratinocyte",
        "Buoyocyte",
        "Glueocyte",
        "Virocyte",
        "Nitrocyte",
        "Stereocyte",
        "Senseocyte",
        "Myocyte",
        "Neurocyte",
        "Secrocyte",
        "Stemocyte",
        "Gamete",
        "Ciliocyte",
    ];
    NAMES
        .get(value.max(0) as usize)
        .copied()
        .unwrap_or("Unknown")
}

fn cell_origin_name(value: i32) -> &'static str {
    match value {
        0 => "Challenge cell",
        1 => "User cell",
        2 => "Contamination cell",
        3 => "Infected challenge cell",
        4 => "Infected user cell",
        5 => "Infected contamination cell",
        6 => "Hybrid cell",
        _ => "Unknown",
    }
}
fn snapshot_for_start(world: &World) -> Snapshot {
    Snapshot {
        cells: Vec::new(),
        food: Vec::new(),
        links: Vec::new(),
        dead: Vec::new(),
        time: world.time,
        radius: world.environment.radius,
        light_angle: world.light_angle,
        light_amount: world.environment.d,
        light_range: world.environment.f,
        max_cells: world.environment.max_cells,
        max_food: world.environment.max_food,
        environment: world.environment.clone(),
        running: false,
        speed: 1.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn priority_slider_matches_apk_geometric_range() {
        assert!((priority_from_fraction(0.0) - 0.1).abs() < 0.00001);
        assert!((priority_from_fraction(0.5) - 1.0).abs() < 0.00001);
        assert!((priority_from_fraction(1.0) - 10.0).abs() < 0.00001);
        for position in [0.0, 1.0 / 24.0, 0.25, 0.5, 0.75, 1.0] {
            let value = priority_from_fraction(position);
            assert!((priority_fraction(value) - position).abs() < 0.00001);
        }
    }

    #[test]
    fn apk_action_slider_preserves_logarithmic_coarse_and_fine_steps() {
        assert_eq!(apk_action_bias(0, 24, true), -1.0);
        assert!((apk_action_bias(1, 24, true) + 0.9793827).abs() < 0.00001);
        assert_eq!(apk_action_bias(12, 24, false), 0.0);
        assert!((apk_action_bias(24, 24, false) - 1.0).abs() < 0.00001);
        for ticks in [24, 1200] {
            for progress in [0, 1, ticks / 4, ticks / 2, ticks - 1, ticks] {
                for positive in [false, true] {
                    let bias = apk_action_bias(progress, ticks, positive);
                    assert_eq!(apk_action_progress(bias, ticks, positive), progress);
                }
            }
        }
    }

    #[test]
    fn copying_and_resetting_modes_preserves_the_initial_choice() {
        let mut genes = default_player_genome();
        genes[1].cell_type = 3;
        genes[1].output_values[2] = 0.73;
        genes[1].flags[4] = false;
        genes[2].flags[4] = true;
        genes[0].flags[4] = false;
        apply_mode_action(&mut genes, 1, ModeAction::Copy(2), 0);
        assert_eq!(genes[2].cell_type, 3);
        assert_eq!(genes[2].output_values[2], 0.73);
        assert!(genes[2].flags[4]);
        apply_mode_action(&mut genes, 1, ModeAction::Reset(2), 0);
        assert_eq!(genes[2].cell_type, 0);
        assert_eq!(genes[2].i, 2);
        assert_eq!(genes[2].j, 2);
        assert!(genes[2].flags[4]);
        assert_eq!(genes.iter().filter(|gene| gene.flags[4]).count(), 1);
    }

    #[test]
    fn wheel_zoom_keeps_the_world_point_under_the_pointer() {
        let viewport_center = Pos2::new(640.0, 400.0);
        let pointer = Pos2::new(870.0, 235.0);
        let mut pan = Vec2::new(35.0, -42.0);
        let mut zoom = 1.5;
        for scroll_delta in [120.0, -160.0, 100_000.0, -100_000.0] {
            let before = (pointer - (viewport_center + pan)) / zoom;
            zoom_at_pointer(&mut zoom, &mut pan, viewport_center, pointer, scroll_delta);
            let after = (pointer - (viewport_center + pan)) / zoom;
            assert!((before - after).length() < 0.001);
        }
    }
}
