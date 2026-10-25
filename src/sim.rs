use crate::{
    bundled, legacy,
    levels::{self, Condition},
    model::{Cell, DeadCell, Environment, Food, Gene, GeneInput, Link, World, fresh_cell_id},
    native,
};
use rand::Rng;
use rayon::prelude::*;
use std::{
    collections::{HashMap, HashSet},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, Sender, TryRecvError},
    },
    thread,
    time::{Duration, Instant},
};

// Cell.a in the APK: sqrt(1.7999999999999998E-4).
const MIN_CELL_RADIUS: f64 = 0.013_416_407_864_998_738;

pub enum Command {
    Play(bool),
    Speed(f64),
    WatchChallenge(Option<(u64, Vec<Condition>, f64)>),
    Step,
    Load(String),
    LoadBundled {
        index: usize,
        as_challenge: bool,
    },
    Save(String),
    Radius(f64),
    MaxCells(usize),
    MaxFood(usize),
    FoodCount(usize),
    EnvironmentSettings(Environment),
    AddCell(
        f64,
        f64,
        Option<u64>,
        Option<Vec<Gene>>,
        Option<[Option<usize>; 18]>,
    ),
    AddFood(f64, f64),
    BoostCell(u64),
    GrabCell(u64),
    MoveCell(u64, f64, f64),
    ReleaseCell(u64),
    RemoveCell(u64),
    NewPlate,
    InspectCell(u64),
    ReplaceGenome(u64, Vec<Gene>, i32),
}

#[derive(Clone)]
pub struct RenderCell {
    pub runtime_id: u64,
    pub x: f32,
    pub y: f32,
    pub radius: f32,
    pub angle: f32,
    pub phase: f32,
    pub reserve: f32,
    pub signals: [f32; 2],
    pub activity: [f32; 2],
    pub animation_phase: f32,
    pub auxiliary: [f32; 4],
    pub mass: f32,
    pub age: f32,
    pub nitrogen: f32,
    pub toxins: f32,
    pub injury: f32,
    pub lift: f32,
    pub signal_levels: [f32; 4],
    pub mirrored: bool,
    pub mutations: i32,
    pub telomeres: i32,
    pub color: [f32; 3],
    pub stored_color: [f32; 3],
    pub cell_type: i32,
    pub origin: i32,
    pub mode: i32,
    /// Inward clipping planes at circle-contact chords, in plate coordinates.
    pub contact_chords: Vec<[f32; 3]>,
}
#[derive(Clone)]
pub struct RenderFood {
    pub x: f32,
    pub y: f32,
    pub energy: f32,
    pub state: f32,
}
#[derive(Clone)]
pub struct RenderLink {
    pub from: [f32; 2],
    pub to: [f32; 2],
    pub left: [f32; 2],
    pub right: [f32; 2],
}
#[derive(Clone)]
pub struct RenderDead {
    pub x: f32,
    pub y: f32,
    pub radius: f32,
    pub age: f32,
    pub color: [f32; 3],
}
#[derive(Clone)]
pub struct Snapshot {
    pub cells: Vec<RenderCell>,
    pub food: Vec<RenderFood>,
    pub links: Vec<RenderLink>,
    pub dead: Vec<RenderDead>,
    pub time: f64,
    pub radius: f64,
    pub light_angle: f64,
    pub light_amount: f64,
    pub light_range: f64,
    pub max_cells: usize,
    pub max_food: usize,
    pub environment: Environment,
    pub running: bool,
    pub speed: f64,
}
pub enum Event {
    Snapshot(Snapshot),
    ChallengeWon(u64),
    Message(String),
    Loaded,
    CellAdded,
    CellAddRejected,
    CellRemoved,
    CellRemoveRejected,
    Inspected(u64, Cell),
}

pub struct Engine {
    pub commands: Sender<Command>,
    pub events: Receiver<Event>,
    snapshot_pending: Arc<AtomicBool>,
}

struct ChallengeGoal {
    run_id: u64,
    conditions: Vec<Condition>,
    hold_time: f64,
    hold_since: Option<f64>,
    won: bool,
}

fn check_challenge_goal(
    goal: &mut Option<ChallengeGoal>,
    world: &World,
    running: &mut bool,
    events: &mpsc::SyncSender<Event>,
) {
    let Some(goal) = goal.as_mut() else {
        return;
    };
    if goal.won {
        return;
    }
    if levels::conditions_met_world(&goal.conditions, world) {
        let since = goal.hold_since.get_or_insert(world.time);
        if world.time - *since >= goal.hold_time {
            goal.won = true;
            *running = false;
            let _ = events.send(Event::ChallengeWon(goal.run_id));
        }
    } else {
        goal.hold_since = None;
    }
}

impl Engine {
    pub fn acknowledge_snapshot(&self) {
        self.snapshot_pending.store(false, Ordering::Release);
    }
}

fn cell_index(world: &World, id: u64) -> Option<usize> {
    world.cells.iter().position(|cell| cell.runtime_id == id)
}

fn cell_by_id_mut(world: &mut World, id: u64) -> Option<&mut Cell> {
    world.cells.iter_mut().find(|cell| cell.runtime_id == id)
}

fn boost_cell(cell: &mut Cell) {
    cell.values[7] = 0.36;
    cell.values[8] = 0.0;
    cell.color = [1.0, 0.0, 1.0];
    cell.tuvp[3] = 1.0;
    if cell.cell_type() == 4 {
        cell.tail[5] = 1.8;
    }
}

fn ensure_unique_cell_ids(world: &mut World) {
    let mut seen = HashSet::with_capacity(world.cells.len());
    for cell in &mut world.cells {
        if cell.runtime_id != 0 && seen.insert(cell.runtime_id) {
            continue;
        }
        loop {
            let id = fresh_cell_id();
            if id != 0 && seen.insert(id) {
                cell.runtime_id = id;
                break;
            }
        }
    }
}

fn synthesize_cell(x: f64, y: f64, genes: Vec<Gene>) -> Option<Cell> {
    if genes.is_empty() {
        return None;
    }
    let mode = genes.iter().rposition(|gene| gene.flags[4]).unwrap_or(0);
    let mass: f64 = if genes[mode].cell_type == 4 {
        1.728
    } else {
        0.288
    };
    let color = genes[mode].color;
    Some(Cell {
        runtime_id: fresh_cell_id(),
        x,
        y,
        values: [
            0.0,
            rand::rng().random_range(-std::f64::consts::PI..=std::f64::consts::PI),
            0.0,
            0.0,
            0.0,
            0.0,
            (mass / 400.0).sqrt().min(0.0297),
            mass,
            0.1,
        ],
        links: Vec::new(),
        genes,
        c: 0,
        x_flag: false,
        color,
        mode: mode as i32,
        e: 1,
        f: 0,
        tuvp: [0.0, 0.0, 0.0, 1.0],
        y_flag: false,
        u: [0.01; 4],
        w: [0.0; 4],
        tail: [0.0; 6],
        g: 0,
        t: -1,
        q: 0.0,
        resting: false,
        visual_phase: 0.0,
        visual_phase2: 0.0,
        mutation_ticks: 0,
    })
}

fn load_bundled(index: usize, as_challenge: bool) -> Result<World, String> {
    let (_, bytes) = bundled::PLATES
        .get(index)
        .ok_or("Unknown bundled plate".to_string())?;
    let mut world = legacy::import(bytes)?;
    if as_challenge {
        // The APK resets provenance and simulation time after loading a level.
        for cell in &mut world.cells {
            cell.e = 0;
        }
        world.time = 0.0;
    }
    Ok(world)
}

pub fn start(mut world: World) -> Engine {
    ensure_unique_cell_ids(&mut world);
    let (commands, command_receiver) = mpsc::channel();
    let (event_sender, events) = mpsc::sync_channel(4);
    let snapshot_pending = Arc::new(AtomicBool::new(false));
    let worker_pending = Arc::clone(&snapshot_pending);
    thread::Builder::new()
        .name("celllab-simulation".into())
        .spawn(move || {
            let mut running = false;
            let mut speed = 1.0;
            let mut accumulator = 0.0;
            let mut last = Instant::now();
            let mut last_snapshot = Instant::now() - Duration::from_secs(1);
            let mut changed = true;
            let mut challenge_goal: Option<ChallengeGoal> = None;
            'worker: loop {
                loop {
                    let command = match command_receiver.try_recv() {
                        Ok(command) => command,
                        Err(TryRecvError::Empty) => break,
                        Err(TryRecvError::Disconnected) => break 'worker,
                    };
                    let result = match command {
                        Command::Play(value) => {
                            running = value;
                            Ok(())
                        }
                        Command::Speed(value) => {
                            if value == f64::INFINITY || (value.is_finite() && value >= 0.1) {
                                speed = value;
                                accumulator = 0.0;
                                Ok(())
                            } else {
                                Err("Speed must be at least 0.1× or Incubate.".into())
                            }
                        }
                        Command::WatchChallenge(watch) => {
                            challenge_goal =
                                watch.map(|(run_id, conditions, hold_time)| ChallengeGoal {
                                    run_id,
                                    conditions,
                                    hold_time,
                                    hold_since: None,
                                    won: false,
                                });
                            Ok(())
                        }
                        Command::Step => {
                            step(&mut world, 0.02);
                            Ok(())
                        }
                        Command::Load(path) => native::load(&path)
                            .map(|loaded| {
                                world = loaded;
                                ensure_unique_cell_ids(&mut world);
                                running = false;
                                challenge_goal = None;
                                let _ = event_sender.send(Event::Loaded);
                            })
                            .map_err(|e| format!("Open failed: {e}")),
                        Command::LoadBundled {
                            index,
                            as_challenge,
                        } => load_bundled(index, as_challenge).map(|loaded| {
                            world = loaded;
                            ensure_unique_cell_ids(&mut world);
                            running = false;
                            challenge_goal = None;
                            let _ = event_sender.send(Event::Loaded);
                        }),
                        Command::Save(path) => native::save(&path, &world)
                            .map(|()| {
                                let _ = event_sender.send(Event::Message(format!("Saved {path}")));
                            })
                            .map_err(|e| format!("Save failed: {e}")),
                        Command::Radius(value) => {
                            world.set_radius(value);
                            Ok(())
                        }
                        Command::MaxCells(value) => {
                            world.set_max_cells(value);
                            Ok(())
                        }
                        Command::MaxFood(value) => {
                            world.set_max_food(value);
                            Ok(())
                        }
                        Command::FoodCount(value) => {
                            set_food_count(&mut world, value);
                            Ok(())
                        }
                        Command::EnvironmentSettings(mut settings) => {
                            if !settings.enabled_types.iter().any(|enabled| *enabled) {
                                settings.enabled_types = world.environment.enabled_types.clone();
                            }
                            settings.radius = world.environment.radius;
                            settings.max_cells = world.environment.max_cells;
                            settings.max_food = world.environment.max_food;
                            world.environment = settings;
                            Ok(())
                        }
                        Command::AddCell(x, y, source, genome, limits) => {
                            if world.environment.max_cells == 0
                                || world.cells.len() > world.environment.max_cells
                            {
                                let _ = event_sender.send(Event::CellAddRejected);
                                let _ = event_sender.try_send(Event::Message(
                                    "Cell limit reached. Increase Maximum cells to place more."
                                        .into(),
                                ));
                                continue;
                            }
                            if x.hypot(y) >= world.environment.radius {
                                let _ = event_sender.send(Event::CellAddRejected);
                                continue;
                            }
                            let synthesized = genome.is_some();
                            let selected = if let Some(genes) = genome {
                                synthesize_cell(x, y, genes)
                            } else {
                                source
                                    .and_then(|id| cell_index(&world, id))
                                    .and_then(|index| world.cells.get(index))
                                    .cloned()
                            };
                            if let Some(mut cell) = selected {
                                cell.runtime_id = fresh_cell_id();
                                if limits.is_some_and(|limits| {
                                    limits.iter().enumerate().any(|(kind, limit)| {
                                        limit.is_some_and(|max| {
                                            cell.genes
                                                .iter()
                                                .filter(|gene| gene.cell_type == kind as i32)
                                                .count()
                                                > max
                                        })
                                    })
                                }) {
                                    let _ = event_sender.send(Event::CellAddRejected);
                                    let _ = event_sender.try_send(Event::Message(
                                        "Genome exceeds a cell type limit for this challenge."
                                            .into(),
                                    ));
                                    continue;
                                }
                                cell.x = x;
                                cell.y = y;
                                cell.values[2] = 0.0;
                                cell.values[3] = 0.0;
                                if !synthesized {
                                    cell.values[8] = 0.0;
                                }
                                cell.links.clear();
                                cell.c = 0;
                                cell.x_flag = false;
                                cell.f = 0;
                                cell.mutation_ticks = 0;
                                cell.e = 1;
                                // The APK replaces a random existing organism when
                                // a player places one on a plate at its cell cap.
                                if world.cells.len() == world.environment.max_cells {
                                    let victim = rand::rng().random_range(0..world.cells.len());
                                    evict_cell_for_placement(&mut world, victim);
                                }
                                world.cells.push(cell);
                                let _ = event_sender.send(Event::CellAdded);
                                Ok(())
                            } else {
                                let _ = event_sender.send(Event::CellAddRejected);
                                Err("No synthesizer genome or selected organism is available."
                                    .into())
                            }
                        }
                        Command::AddFood(x, y) => {
                            if x.hypot(y) >= world.environment.radius
                                || world.food.len() >= world.environment.max_food
                            {
                                Err(
                                    "Food is outside the plate or the food limit has been reached."
                                        .into(),
                                )
                            } else {
                                world.food.push(Food {
                                    x: x as f32,
                                    y: y as f32,
                                    energy: 1.152,
                                    vx: 0.0,
                                    vy: 0.0,
                                    state: 0.0,
                                });
                                Ok(())
                            }
                        }
                        Command::BoostCell(id) => {
                            if let Some(cell) = cell_by_id_mut(&mut world, id) {
                                boost_cell(cell);
                            }
                            Ok(())
                        }
                        Command::GrabCell(id) => {
                            if let Some(cell) = cell_by_id_mut(&mut world, id) {
                                cell.f = 100;
                            }
                            Ok(())
                        }
                        Command::MoveCell(id, x, y) => {
                            if x.hypot(y) >= world.environment.radius {
                                Ok(())
                            } else if let Some(cell) = cell_by_id_mut(&mut world, id) {
                                cell.x = x;
                                cell.y = y;
                                cell.values[2] = 0.0;
                                cell.values[3] = 0.0;
                                Ok(())
                            } else {
                                Err("Organism no longer exists.".into())
                            }
                        }
                        Command::ReleaseCell(id) => {
                            if let Some(cell) = cell_by_id_mut(&mut world, id) {
                                if cell.f >= 100 {
                                    cell.f = 0;
                                }
                            }
                            Ok(())
                        }
                        Command::RemoveCell(id) => {
                            if let Some(index) = cell_index(&world, id) {
                                compact_cells(&mut world, |i, _| i != index);
                                let _ = event_sender.send(Event::CellRemoved);
                            } else {
                                let _ = event_sender.send(Event::CellRemoveRejected);
                            }
                            Ok(())
                        }
                        Command::NewPlate => {
                            world.time = 0.0;
                            world.cells.clear();
                            world.food.clear();
                            running = false;
                            challenge_goal = None;
                            let _ = event_sender.send(Event::Loaded);
                            Ok(())
                        }
                        Command::InspectCell(id) => {
                            if let Some(cell) = cell_by_id_mut(&mut world, id) {
                                let _ = event_sender.send(Event::Inspected(id, cell.clone()));
                            }
                            Ok(())
                        }
                        Command::ReplaceGenome(id, genes, mode) => {
                            if let Some(cell) = cell_by_id_mut(&mut world, id) {
                                if !genes.is_empty() {
                                    cell.genes = genes;
                                    cell.mode = mode.clamp(0, cell.genes.len() as i32 - 1);
                                    cell.color = cell.genes[cell.mode as usize].color;
                                }
                            }
                            Ok(())
                        }
                    };
                    match result {
                        Ok(()) => {
                            check_challenge_goal(
                                &mut challenge_goal,
                                &world,
                                &mut running,
                                &event_sender,
                            );
                            changed = true;
                        }
                        Err(error) => {
                            let _ = event_sender.send(Event::Message(error));
                        }
                    }
                }
                let now = Instant::now();
                let elapsed = now.duration_since(last).as_secs_f64().min(0.25);
                last = now;
                const BURST_STEPS: usize = 8;
                let mut stepped = false;
                if running {
                    if speed == f64::INFINITY {
                        for _ in 0..BURST_STEPS {
                            step(&mut world, 0.02);
                            check_challenge_goal(
                                &mut challenge_goal,
                                &world,
                                &mut running,
                                &event_sender,
                            );
                            if !running {
                                break;
                            }
                        }
                        stepped = true;
                        accumulator = 0.0;
                    } else {
                        // If the requested rate outruns the PC, keep running
                        // flat out without building an unbounded time debt.
                        accumulator =
                            (accumulator + elapsed * speed).min(BURST_STEPS as f64 * 0.02);
                        let mut iterations = 0;
                        while accumulator >= 0.02 && iterations < BURST_STEPS {
                            step(&mut world, 0.02);
                            check_challenge_goal(
                                &mut challenge_goal,
                                &world,
                                &mut running,
                                &event_sender,
                            );
                            accumulator -= 0.02;
                            iterations += 1;
                            stepped = true;
                            if !running {
                                break;
                            }
                        }
                    }
                    if stepped {
                        changed = true;
                    }
                } else {
                    accumulator = 0.0;
                }
                if changed
                    && !worker_pending.load(Ordering::Acquire)
                    && last_snapshot.elapsed() >= Duration::from_millis(50)
                {
                    // A full render snapshot is expensive on large plates. If
                    // the UI queue is full, retry on the next snapshot tick.
                    // Keep only one snapshot in flight while the UI lags.
                    last_snapshot = Instant::now();
                    worker_pending.store(true, Ordering::Release);
                    if event_sender
                        .try_send(Event::Snapshot(snapshot(&world, running, speed)))
                        .is_ok()
                    {
                        changed = false;
                    } else {
                        worker_pending.store(false, Ordering::Release);
                    }
                }
                if !stepped {
                    thread::sleep(Duration::from_millis(3));
                }
            }
        })
        .expect("simulation thread");
    Engine {
        commands,
        events,
        snapshot_pending,
    }
}

fn snapshot(world: &World, running: bool, speed: f64) -> Snapshot {
    let mut render_cells: Vec<RenderCell> = world
        .cells
        .iter()
        .map(|c| RenderCell {
            runtime_id: c.runtime_id,
            x: c.x as f32,
            y: c.y as f32,
            radius: c.values[6].abs() as f32,
            angle: c.values[0] as f32,
            phase: c.values[1] as f32,
            reserve: c.tail[5],
            signals: [c.w[0], c.w[1]],
            activity: if c.cell_type() == 15 {
                let first = (c.signal(5) + 0.5).max(0.0);
                let second = (c.signal(6) + 0.5).max(0.0);
                [
                    first / (1.0 + second * second) * std::f32::consts::PI,
                    second / (1.0 + first * first) * std::f32::consts::PI,
                ]
            } else {
                [c.tail[2], c.tail[3]]
            },
            animation_phase: c.visual_phase,
            auxiliary: [
                c.tuvp[0] as f32,
                c.tuvp[1] as f32,
                if c.cell_type() == 17 {
                    c.y_flag as u8 as f32
                } else {
                    c.tuvp[2] as f32
                },
                c.visual_phase2,
            ],
            mass: c.values[7] as f32,
            age: c.values[8] as f32,
            nitrogen: c.tuvp[3] as f32,
            toxins: c.tail[0],
            injury: c.tail[1],
            lift: c.q as f32,
            signal_levels: c.u,
            mirrored: c.y_flag,
            mutations: c.g,
            telomeres: c.t,
            color: [
                c.color[0] + c.tail[0] * 0.1,
                c.color[1] + c.tail[1],
                c.color[2],
            ],
            stored_color: c.color,
            cell_type: c.cell_type(),
            origin: c.e,
            mode: c.mode,
            contact_chords: Vec::new(),
        })
        .collect();
    add_contact_chords(&mut render_cells, |index| !world.cells[index].x_flag);
    Snapshot {
        cells: render_cells,
        food: world
            .food
            .iter()
            .map(|f| RenderFood {
                x: f.x,
                y: f.y,
                energy: f.energy,
                state: f.state,
            })
            .collect(),
        links: world
            .cells
            .iter()
            .enumerate()
            .flat_map(|(index, cell)| {
                cell.links
                    .iter()
                    .take(cell.c.max(0) as usize)
                    .filter_map(move |link| {
                        let other = if link.target == -1 {
                            None
                        } else {
                            let target = usize::try_from(link.target).ok()?;
                            if target >= index {
                                return None;
                            }
                            Some(world.cells.get(target)?)
                        };
                        render_link_geometry(cell, link, other)
                    })
            })
            .collect(),
        dead: world
            .dead
            .iter()
            .map(|cell| RenderDead {
                x: cell.x,
                y: cell.y,
                radius: cell.radius,
                age: cell.age,
                color: cell.color,
            })
            .collect(),
        time: world.time,
        radius: world.environment.radius,
        light_angle: world.light_angle,
        light_amount: world.environment.d,
        light_range: world.environment.f,
        max_cells: world.environment.max_cells,
        max_food: world.environment.max_food,
        environment: world.environment.clone(),
        running,
        speed,
    }
}

/// CellWorld.i() records the shared chord of each overlapping pair for its
/// contact-shaped cell meshes. A sparse grid keeps snapshot construction near
/// linear on large plates while retaining the APK's 20-contact limit.
fn add_contact_chords(cells: &mut [RenderCell], mut active: impl FnMut(usize) -> bool) {
    let max_radius = cells.iter().map(|cell| cell.radius).fold(0.03, f32::max);
    let bin = (2.0 * max_radius).max(0.06);
    let mut keys = vec![(0_i32, 0_i32); cells.len()];
    let mut active_flags = vec![false; cells.len()];
    let mut indices = Vec::new();
    let (mut min_x, mut min_y) = (i32::MAX, i32::MAX);
    let (mut max_x, mut max_y) = (i32::MIN, i32::MIN);
    for (index, cell) in cells.iter().enumerate() {
        if !active(index) {
            continue;
        }
        active_flags[index] = true;
        indices.push(index);
        let key = ((cell.x / bin).floor() as i32, (cell.y / bin).floor() as i32);
        keys[index] = key;
        min_x = min_x.min(key.0);
        min_y = min_y.min(key.1);
        max_x = max_x.max(key.0);
        max_y = max_y.max(key.1);
    }
    if indices.len() < 2 {
        return;
    }
    let width = (i64::from(max_x) - i64::from(min_x) + 1) as usize;
    let height = (i64::from(max_y) - i64::from(min_y) + 1) as usize;
    let slots = width.checked_mul(height).unwrap_or(usize::MAX);
    if slots > indices.len().saturating_mul(8) || slots > 1_000_000 {
        add_contact_chords_hash(cells, |index| active_flags[index]);
        return;
    }
    let bucket = |key: (i32, i32)| (key.1 - min_y) as usize * width + (key.0 - min_x) as usize;
    let mut offsets = vec![0_usize; slots + 1];
    for &index in &indices {
        offsets[bucket(keys[index]) + 1] += 1;
    }
    for slot in 1..=slots {
        offsets[slot] += offsets[slot - 1];
    }
    let mut cursor = offsets[..slots].to_vec();
    let mut members = vec![0_usize; indices.len()];
    for &index in &indices {
        let slot = bucket(keys[index]);
        members[cursor[slot]] = index;
        cursor[slot] += 1;
    }
    for &index in &indices {
        let key = keys[index];
        for dy in -1..=1_i64 {
            let y = i64::from(key.1) + dy;
            if y < i64::from(min_y) || y > i64::from(max_y) {
                continue;
            }
            for dx in -1..=1_i64 {
                let x = i64::from(key.0) + dx;
                if x < i64::from(min_x) || x > i64::from(max_x) {
                    continue;
                }
                let slot =
                    (y - i64::from(min_y)) as usize * width + (x - i64::from(min_x)) as usize;
                for &other in &members[offsets[slot]..offsets[slot + 1]] {
                    if other >= index {
                        break;
                    }
                    if cells[index].contact_chords.len() >= 20
                        || cells[other].contact_chords.len() >= 20
                    {
                        continue;
                    }
                    let ox = cells[other].x - cells[index].x;
                    let oy = cells[other].y - cells[index].y;
                    let distance_sq = ox * ox + oy * oy;
                    let r1 = cells[index].radius;
                    let r2 = cells[other].radius;
                    let distance = distance_sq.sqrt();
                    if distance <= (r1 - r2).abs() || distance >= r1 + r2 {
                        continue;
                    }
                    let nx = ox / distance;
                    let ny = oy / distance;
                    let first_depth = (distance_sq + r1 * r1 - r2 * r2) / (2.0 * distance);
                    let second_depth = distance - first_depth;
                    cells[index].contact_chords.push([nx, ny, first_depth]);
                    cells[other].contact_chords.push([-nx, -ny, second_depth]);
                }
            }
        }
    }
}

fn add_contact_chords_hash(cells: &mut [RenderCell], mut active: impl FnMut(usize) -> bool) {
    let max_radius = cells.iter().map(|cell| cell.radius).fold(0.03, f32::max);
    let bin = (2.0 * max_radius).max(0.06);
    let mut buckets: HashMap<(i32, i32), Vec<usize>> = HashMap::new();
    for index in 0..cells.len() {
        if !active(index) {
            continue;
        }
        let first = &cells[index];
        let key = (
            (first.x / bin).floor() as i32,
            (first.y / bin).floor() as i32,
        );
        for dy in -1..=1 {
            for dx in -1..=1 {
                if let Some(neighbors) = buckets.get(&(key.0 + dx, key.1 + dy)) {
                    for &other in neighbors {
                        if cells[index].contact_chords.len() >= 20
                            || cells[other].contact_chords.len() >= 20
                        {
                            continue;
                        }
                        let ox = cells[other].x - cells[index].x;
                        let oy = cells[other].y - cells[index].y;
                        let distance_sq = ox * ox + oy * oy;
                        let r1 = cells[index].radius;
                        let r2 = cells[other].radius;
                        let distance = distance_sq.sqrt();
                        if distance <= (r1 - r2).abs() || distance >= r1 + r2 {
                            continue;
                        }
                        let nx = ox / distance;
                        let ny = oy / distance;
                        let first_depth = (distance_sq + r1 * r1 - r2 * r2) / (2.0 * distance);
                        let second_depth = distance - first_depth;
                        cells[index].contact_chords.push([nx, ny, first_depth]);
                        cells[other].contact_chords.push([-nx, -ny, second_depth]);
                    }
                }
            }
        }
        buckets.entry(key).or_default().push(index);
    }
}

pub fn preview_snapshot(world: &World) -> Snapshot {
    snapshot(world, false, 0.0)
}

/// CellWorld.c() builds a short central segment and two triangles for each link.
/// Attachment offsets are recomputed from the same angle/radius formula used by
/// CellWorld's physics pass because the APK does not serialize its g/h/i/j fields.
fn render_link_geometry(cell: &Cell, link: &Link, other: Option<&Cell>) -> Option<RenderLink> {
    const CELL_CORE: f64 = 0.013_416_407_864_998_738;
    let width = ((CELL_CORE as f32) * 0.7_f32) as f64;
    let mut from_anchor = link.c;
    let mut to_anchor = link.d;
    if let Some(other) = other {
        if cell.cell_type() == 12 {
            from_anchor += link.c.cos() * cell.tail[3] as f64;
        }
        if other.cell_type() == 12 {
            to_anchor += link.d.cos() * other.tail[3] as f64;
        }
    }
    let angle = cell.values[0] + from_anchor;
    let inset = (cell.values[6] - CELL_CORE) * 0.8;
    let from = [cell.x + inset * angle.cos(), cell.y + inset * angle.sin()];
    let (to, left, right) = if let Some(other) = other {
        let other_angle = other.values[0] + to_anchor;
        let other_inset = (other.values[6] - CELL_CORE) * 0.8;
        let to = [
            other.x + other_inset * other_angle.cos(),
            other.y + other_inset * other_angle.sin(),
        ];
        let dx = other.x - cell.x;
        let dy = other.y - cell.y;
        let distance = dx.hypot(dy);
        if distance < 1.0e-12 {
            return None;
        }
        let weight =
            (cell.values[6] + 0.5 * (distance - cell.values[6] - other.values[6])) / distance;
        let center = [
            cell.x * weight + other.x * (1.0 - weight),
            other.y * (1.0 - weight) + cell.y * weight,
        ];
        let lateral = 0.5 * width / distance;
        (
            to,
            [center[0] - dy * lateral, center[1] + dx * lateral],
            [center[0] + dy * lateral, center[1] - dx * lateral],
        )
    } else {
        let to = [link.e, link.f];
        let distance = cell.x.hypot(cell.y);
        if distance < 1.0e-12 {
            return None;
        }
        let center = [0.5 * (link.e + cell.x), 0.5 * (link.f + cell.y)];
        let lateral = 0.5 * width / distance;
        (
            to,
            [center[0] + cell.y * lateral, center[1] - cell.x * lateral],
            [center[0] - cell.y * lateral, center[1] + cell.x * lateral],
        )
    };
    let values = [
        from[0], from[1], to[0], to[1], left[0], left[1], right[0], right[1],
    ];
    values
        .iter()
        .all(|value| value.is_finite())
        .then_some(RenderLink {
            from: [from[0] as f32, from[1] as f32],
            to: [to[0] as f32, to[1] as f32],
            left: [left[0] as f32, left[1] as f32],
            right: [right[0] as f32, right[1] as f32],
        })
}

pub fn set_food_count(world: &mut World, requested: usize) {
    let requested = requested.min(1_000_000);
    world.food.truncate(requested);
    let mut rng = rand::rng();
    let radius = world.environment.radius as f32;
    while world.food.len() < requested {
        let angle = rng.random::<f32>() * std::f32::consts::TAU;
        let distance = rng.random::<f32>().sqrt() * radius * 0.98;
        world.food.push(Food {
            x: angle.cos() * distance,
            y: angle.sin() * distance,
            energy: world.environment.b.max(0.01) as f32,
            vx: 0.0,
            vy: 0.0,
            state: 0.0,
        });
    }
}

// The independent integration phase uses Rayon. Interactions are applied after
// all positions have advanced, so workers never mutate shared organisms.
#[derive(Default)]
pub struct StepTimings {
    pub integration: u128,
    pub contacts: u128,
    pub links: u128,
    pub contact_grid: u128,
    pub contact_search: u128,
    pub contact_effects: u128,
    pub contact_marking: u128,
    pub contact_metadata: u128,
    pub contact_pairs: u128,
    pub contact_apply: u128,
    pub cleanup: u128,
    pub food: u128,
    pub signals: u128,
    pub metabolism: u128,
    pub resource_exchange: u128,
    pub sensing: u128,
    pub reproduction: u128,
}

impl std::ops::AddAssign for StepTimings {
    fn add_assign(&mut self, other: Self) {
        self.integration += other.integration;
        self.contacts += other.contacts;
        self.links += other.links;
        self.contact_grid += other.contact_grid;
        self.contact_search += other.contact_search;
        self.contact_effects += other.contact_effects;
        self.contact_marking += other.contact_marking;
        self.contact_metadata += other.contact_metadata;
        self.contact_pairs += other.contact_pairs;
        self.contact_apply += other.contact_apply;
        self.cleanup += other.cleanup;
        self.food += other.food;
        self.signals += other.signals;
        self.metabolism += other.metabolism;
        self.resource_exchange += other.resource_exchange;
        self.sensing += other.sensing;
        self.reproduction += other.reproduction;
    }
}

pub fn step(world: &mut World, dt: f64) -> StepTimings {
    step_inner(world, dt, false, &mut rand::rng())
}

pub fn step_preview(world: &mut World, dt: f64, rng: &mut impl Rng) -> StepTimings {
    step_inner(world, dt, true, rng)
}

fn step_inner(world: &mut World, dt: f64, preview: bool, rng: &mut impl Rng) -> StepTimings {
    let started = Instant::now();
    world.time += dt;
    world.light_angle += world.environment.e * dt;
    if !preview {
        spawn_food(world, dt);
        spawn_random_cells(world);
    }
    let radius = world.environment.radius;
    let friction = world.environment.m;
    let static_friction = world.environment.l;
    let fluid_drag = world.environment.g;
    world.cells.par_iter_mut().for_each(|cell| {
        let v = &mut cell.values;
        if v[8] > 5.0 * dt && v[6] > 0.0 {
            v[6] += (v[7] / 400.0 / v[6] - v[6]) * 5.0 * dt;
            v[6] = v[6].min(0.03);
        }
        // CellWorld.f() holds grabbed cells (F >= 100) at their saved position
        // and primary angle. Their secondary angle and internal state still run.
        if cell.f < 100 {
            cell.x += v[2] * dt;
            cell.y += v[3] * dt;
            v[0] = (v[0] + v[4] * dt + std::f64::consts::PI).rem_euclid(std::f64::consts::TAU)
                - std::f64::consts::PI;
        }
        v[1] += v[5] * dt;
        if v[1] > std::f64::consts::TAU {
            v[1] -= 2.0 * std::f64::consts::TAU;
        } else if v[1] < -std::f64::consts::TAU {
            v[1] += 2.0 * std::f64::consts::TAU;
        }
        if v[6] > 0.03 || v[6] < MIN_CELL_RADIUS || v[7] < 0.072 || cell.tail[1] > 1.0 {
            cell.x_flag = true;
        }
    });
    let integrated = Instant::now();
    // CellWorld.b() divides and removes invalid cells before it builds the
    // food and contact grids. New daughters can feed and collide this step.
    reproduce(world);
    compact_cells_with_recycling(
        world,
        |_, cell| {
            cell.x.hypot(cell.y) < radius
                && cell.values[7] >= 0.072
                && cell.values[6] >= MIN_CELL_RADIUS
                && cell.values[6] <= 0.03
                && cell.tail[1] <= 1.0
                && !cell.x_flag
        },
        !preview,
    );
    let reproduced = Instant::now();
    if !preview {
        world.food.par_iter_mut().for_each(|food| {
            food.energy -= (0.006 * dt) as f32;
        });
        eat_food(world, dt);
        handle_food_edges(world);
        let remove_food_at_edge = world.environment.y && world.environment.x;
        world.food.retain(|f| {
            f.energy > 0.0 && (!remove_food_at_edge || (f.x as f64).hypot(f.y as f64) < radius)
        });
    }
    // CellWorld.h(double) ages debris after division and food updates, so
    // cells removed by this step's early cleanup already begin to fade.
    world.dead.retain_mut(|cell| {
        cell.age += (0.5 * dt) as f32;
        cell.age <= (cell.radius / 0.03).powi(2)
    });
    let fed = Instant::now();
    solve_links(world, dt);
    let linked = Instant::now();
    let mut contact_profile = ContactTimings::default();
    let mut ciliary_contacts =
        solve_contacts_profile(world, dt, preview, rng, &mut contact_profile);
    ciliary_contacts.extend(apply_plate_edge_force(world, dt));
    let gravity = world.environment.h;
    let gravity_gradient = world.environment.j;
    let gravity_strength = world.environment.i;
    for cell in &mut world.cells {
        if !preview {
            let buoyancy = if cell.cell_type() == 6 {
                cell.signal(1) as f64
            } else {
                0.0
            };
            cell.values[3] +=
                (gravity + gravity_gradient * cell.y / radius - buoyancy) * gravity_strength * dt;
            if cell.cell_type() == 1 && cell.values[7] > 0.0 {
                let thrust = cell.signal(0) as f64;
                cell.values[2] += cell.values[0].cos() * thrust * dt / cell.values[7];
                cell.values[3] += cell.values[0].sin() * thrust * dt / cell.values[7];
            }
        }
        apply_cell_fluid_drag(cell, fluid_drag, dt);
        apply_cell_friction(cell, friction, static_friction, dt);
    }
    apply_ciliary_contacts(world, &mut ciliary_contacts);
    let contacted = Instant::now();
    // Contact deaths remain flagged until the next update's CellWorld.j().
    let cleaned = contacted;
    let resources = capture_resource_states(world);
    let environment = &world.environment;
    let light_angle = world.light_angle;
    world.cells.par_iter_mut().for_each(|cell| {
        cell.values[8] += dt;
        let cell_type = cell.cell_type();
        if cell_type == 15 && cell.values[8] > 0.5 {
            let next = if cell.signal(5) > 0.5 {
                cell.active_gene().map(|g| g.output_modes[0])
            } else if cell.signal(6) > 0.5 {
                cell.active_gene().map(|g| g.output_modes[1])
            } else {
                None
            };
            if let Some(next) = next {
                cell.mode = next.clamp(0, cell.genes.len().saturating_sub(1) as i32);
                cell.values[8] = 0.0;
            }
        }
        crate::mutation::tick(cell, environment, dt);
        if let Some(color) = cell.active_gene().map(|gene| gene.color) {
            for channel in 0..3 {
                cell.color[channel] += (color[channel] - cell.color[channel]) * (8.0 * dt) as f32;
            }
        }
        let thrust = if cell_type == 1 {
            cell.signal(0) as f64
        } else {
            0.0
        };
        match cell_type {
            1 => {
                cell.visual_phase += (thrust * dt) as f32;
                if cell.visual_phase > 0.558_505_36 {
                    cell.visual_phase = cell.visual_phase.rem_euclid(0.558_505_36);
                }
            }
            7 => {
                let spin = cell.tuvp[0];
                cell.tuvp[0] += (-cell.values[4] * cell.values[6] - spin * 100.0 * spin.abs()) * dt;
                let motion = cell.tuvp[1].hypot(cell.tuvp[2]);
                cell.tuvp[1] += (-cell.values[2] - cell.tuvp[1] * 100.0 * motion) * dt;
                cell.tuvp[2] += (-cell.values[3] - cell.tuvp[2] * 100.0 * motion) * dt;
            }
            17 => {
                cell.visual_phase =
                    wrap_ciliary_phase(cell.visual_phase + cell.signal(2) * dt as f32 * 0.3);
                cell.visual_phase2 =
                    wrap_ciliary_phase(cell.visual_phase2 + cell.signal(4) * dt as f32 * 0.3);
            }
            _ => {}
        }
        if cell_type == 12 {
            let targets = [
                cell.signal(3),
                if cell.y_flag {
                    -cell.signal(2)
                } else {
                    cell.signal(2)
                },
                cell.signal(4),
            ];
            let change = targets
                .iter()
                .enumerate()
                .map(|(i, target)| {
                    let difference = *target - cell.tail[i + 2];
                    cell.tail[i + 2] = *target;
                    (difference * difference) as f64
                })
                .sum::<f64>();
            cell.values[7] -= 0.00008 * change / dt.max(1.0e-6);
        }
        let cilia_cost = if cell_type == 17 {
            0.001 * (cell.signal(2).abs() + cell.signal(4).abs()) as f64 * dt
        } else {
            0.0
        };
        let v = &mut cell.values;
        if matches!(cell_type, 2 | 6 | 16) {
            let delta = v[4] - v[5];
            v[5] += delta * delta.abs() * dt;
        }
        if cell_type == 2 {
            let light = light_at(
                cell.x,
                cell.y,
                radius,
                light_angle,
                environment.d,
                environment.f,
            );
            v[7] += 400.0 * light * v[6] * v[6] * dt;
        }
        // The APK's GenomePreviewView feeds growth-capable cells while
        // simulating an isolated genome without a food source.
        if preview && matches!(cell_type, 0 | 2 | 3 | 7) {
            v[7] += 280.0 * v[6] * v[6] * dt;
        }
        const COST: [f64; 18] = [
            0.1, 0.1, 0.25, 0.1, 0.005, 0.01, 0.02, 0.01, 0.2, 0.02, 0.01, 0.01, 0.02, 0.02, 0.03,
            0.02, 0.005, 0.005,
        ];
        let cost = COST.get(cell_type.max(0) as usize).copied().unwrap_or(0.1);
        v[7] -= cost
            * (1.38571428571 - 1.28571428571 * environment.n as f64)
            * 400.0
            * (v[6] + 0.0075)
            * v[6]
            * dt;
        if cell_type == 1 {
            v[7] -= thrust * (0.008 + thrust * 0.0409756) * dt;
        }
        v[7] -= cilia_cost;
        if environment.v && v[8] > 240.0 {
            cell.x_flag = true;
        }
    });
    let metabolized = Instant::now();
    exchange_link_resources(world, dt, resources);
    let exchanged = Instant::now();
    crate::sensing::update_signals(world, dt);
    let signaled = Instant::now();
    StepTimings {
        integration: integrated.duration_since(started).as_nanos(),
        contacts: contacted.duration_since(fed).as_nanos(),
        links: linked.duration_since(fed).as_nanos(),
        contact_grid: contact_profile.grid,
        contact_search: contact_profile.search,
        contact_effects: contact_profile.effects,
        contact_marking: contact_profile.marking,
        contact_metadata: contact_profile.metadata,
        contact_pairs: contact_profile.pairs,
        contact_apply: contact_profile.apply,
        cleanup: cleaned.duration_since(contacted).as_nanos(),
        food: fed.duration_since(reproduced).as_nanos(),
        signals: signaled.duration_since(cleaned).as_nanos(),
        metabolism: metabolized.duration_since(cleaned).as_nanos(),
        resource_exchange: exchanged.duration_since(metabolized).as_nanos(),
        sensing: signaled.duration_since(exchanged).as_nanos(),
        reproduction: reproduced.duration_since(integrated).as_nanos(),
    }
}

/// CellWorld keeps the two Ciliocyte texture phases signed so reversing the
/// programmed output reverses the cilia animation.
fn wrap_ciliary_phase(value: f32) -> f32 {
    const PERIOD: f32 = 0.558_505_36;
    if value.abs() > PERIOD {
        value % PERIOD
    } else {
        value
    }
}

/// CellWorld.d(int) accumulates fluid drag using Environment.g. The force is
/// integrated in CellWorld.c(double), after feeding and before metabolism.
fn apply_cell_fluid_drag(cell: &mut Cell, fluid_drag: f64, dt: f64) {
    if fluid_drag == 0.0 || dt == 0.0 {
        return;
    }
    let radius = cell.values[6];
    let mass = cell.values[7];
    if mass <= 0.0 || radius <= 0.0 {
        return;
    }
    let force_coefficient = radius * fluid_drag;
    let corrected_force = force_coefficient * (1.0 - dt * force_coefficient * 0.5 / mass);
    let linear_factor = 1.0 - corrected_force * dt / mass;
    cell.values[2] *= linear_factor;
    cell.values[3] *= linear_factor;
    cell.values[4] *= 1.0 - radius * fluid_drag * dt / (1.4 * mass);
}

/// CellWorld.c(double): Environment.m applies surface friction after forces.
/// The linear and angular terms share a velocity-dependent split.
fn apply_cell_friction(cell: &mut Cell, friction: f64, static_friction: f64, dt: f64) {
    if dt == 0.0 {
        return;
    }
    let modifier = (1.0 - 3.0 * cell.q).clamp(0.0, 4.0);
    if cell.resting {
        let limit = static_friction * cell.values[7] * modifier * dt;
        let speed_squared = cell.values[2] * cell.values[2] + cell.values[3] * cell.values[3];
        if speed_squared <= limit * limit && cell.values[4].abs() <= limit * (2.0 / 3.0) {
            cell.values[2] = 0.0;
            cell.values[3] = 0.0;
            cell.values[4] = 0.0;
            return;
        }
        cell.resting = false;
    }
    let radius = cell.values[6].abs().max(1.0e-9);
    let speed = cell.values[2].hypot(cell.values[3]);
    let angular = cell.values[4];
    let (linear_share, angular_share) = if angular != 0.0 {
        let ratio = speed / (radius * angular.abs());
        let square = ratio * ratio;
        let linear = (0.1 * ratio + square) / (0.1 * ratio + square + 0.15);
        let angular = (1.0 - (square + ratio) / (ratio + square + 5.0)) * 0.65;
        if linear.is_finite() && angular.is_finite() {
            (linear, angular)
        } else {
            (1.0, 0.0)
        }
    } else {
        (1.0, 0.0)
    };
    let linear_step = linear_share * friction * modifier * dt;
    let linear_stopped = speed <= linear_step;
    if linear_stopped {
        cell.values[2] = 0.0;
        cell.values[3] = 0.0;
    } else if speed > 0.0 {
        let scale = 1.0 - linear_step / speed;
        cell.values[2] *= scale;
        cell.values[3] *= scale;
    }
    let angular_step = angular_share * friction * dt / (0.7 * radius * radius);
    let angular_stopped = angular.abs() <= angular_step;
    if angular_stopped {
        cell.values[4] = 0.0;
    } else {
        cell.values[4] -= angular.signum() * angular_step;
    }
    if linear_stopped && angular_stopped {
        cell.resting = true;
    }
}

/// CellWorld.d(int): links apply spring force, torque, and friction strain.
/// Forces use the pre-step velocities and are integrated together, as in c().
fn solve_links(world: &mut World, dt: f64) {
    if dt == 0.0 || world.cells.is_empty() {
        return;
    }
    const CELL_RADIUS: f64 = 0.013_416_407_864_998_738;
    let mut strain_force: Vec<f64> = world
        .cells
        .iter()
        .map(|cell| -0.300_000_011_920_928_96 * cell.q)
        .collect();
    let mut forces = vec![[0.0_f64; 3]; world.cells.len()];
    let mut broken = Vec::new();
    for (index, cell) in world.cells.iter().enumerate() {
        for link in cell.links.iter().take(cell.c.max(0) as usize) {
            let target = link.target;
            if target == -1 {
                let radius = cell.values[6];
                let mass = cell.values[7];
                if radius <= 0.0 || mass <= 0.0 {
                    continue;
                }
                let angle = cell.values[0] + link.c;
                let (sine, cosine) = angle.sin_cos();
                let ax = cosine * radius * 0.6;
                let ay = sine * radius * 0.6;
                let error_x = link.e - ax - cell.x;
                let error_y = link.f - ay - cell.y;
                if error_x * error_x + error_y * error_y > 4.0 * radius * radius {
                    broken.push((index, -1));
                    continue;
                }
                let contact_vx = cell.values[2] - cell.values[4] * ay;
                let contact_vy = cell.values[3] + cell.values[4] * ax;
                let damping = 50.0 * radius;
                let damping = damping * (1.0 - dt * damping * 0.5 / mass);
                let fx = -contact_vx * damping + error_x * 90.0;
                let fy = -contact_vy * damping + error_y * 90.0;
                forces[index][0] += fx;
                forces[index][1] += fy;
                forces[index][2] += ax * fy - ay * fx;
                continue;
            }
            if target < 0 || target as usize >= index {
                continue;
            }
            let other = &world.cells[target as usize];
            let r1 = cell.values[6];
            let r2 = other.values[6];
            let m1 = cell.values[7];
            let m2 = other.values[7];
            if r1 <= 0.0 || r2 <= 0.0 || m1 <= 0.0 || m2 <= 0.0 {
                continue;
            }
            let min_radius = r1.min(r2);
            let mut depth = min_radius * 0.4;
            let mut angle1 = link.c;
            let mut angle2 = link.d;
            let mut difference = other.q - cell.q;
            if cell.cell_type() == 12 {
                difference -= cell.tail[4] as f64 * link.c.cos();
                let facing = link.c.cos();
                depth += facing * facing * r1 * cell.tail[2] as f64;
                angle1 += facing * cell.tail[3] as f64;
            }
            if other.cell_type() == 12 {
                difference += other.tail[4] as f64 * link.d.cos();
                let facing = link.d.cos();
                depth += facing * facing * r2 * other.tail[2] as f64;
                angle2 += facing * other.tail[3] as f64;
            }
            depth = depth.clamp(0.0, min_radius);
            let (sin1, cos1) = (cell.values[0] + angle1).sin_cos();
            let (sin2, cos2) = (other.values[0] + angle2).sin_cos();
            let ax1 = cos1 * (r1 - depth + link.l as f64);
            let ay1 = sin1 * (r1 - depth + link.l as f64);
            let ax2 = cos2 * (r2 - depth + link.l as f64);
            let ay2 = sin2 * (r2 - depth + link.l as f64);
            let error_x = other.x + ax2 - cell.x - ax1;
            let error_y = other.y + ay2 - cell.y - ay1;
            let maximum = r1.max(r2);
            if error_x * error_x + error_y * error_y > 4.0 * maximum * maximum {
                broken.push((index, target));
                continue;
            }
            let contact_v1x = cell.values[2] - cell.values[4] * ay1;
            let contact_v1y = cell.values[3] + cell.values[4] * ax1;
            let contact_v2x = other.values[2] - other.values[4] * ay2;
            let contact_v2y = other.values[3] + other.values[4] * ax2;
            let damping = 25.0 * m1 * m2 / (m1 + m2) / (cell.c.max(other.c) as f64);
            let stretch = 1.0 + 2.0 * (error_x * error_x + error_y * error_y) / 0.0036;
            let stiffness = 30.0 * CELL_RADIUS / (CELL_RADIUS + 2.0 * link.l as f64);
            let fx = (contact_v2x - contact_v1x) * damping + error_x * stretch * stiffness;
            let fy = (contact_v2y - contact_v1y) * damping + error_y * stretch * stiffness;
            let midpoint_x = 0.5 * (cell.x + ax1 + other.x + ax2);
            let midpoint_y = 0.5 * (cell.y + ay1 + other.y + ay2);
            let bend_area = ay1 * ax2 - ax1 * ay2;
            let bend = bend_area.abs().sqrt() * bend_area.signum() * min_radius * link.k as f64;
            let first_torque = (midpoint_x - cell.x) * fy - (midpoint_y - cell.y) * fx + bend;
            let second_torque = -((midpoint_x - other.x) * fy - (midpoint_y - other.y) * fx) - bend;
            forces[index][0] += fx;
            forces[index][1] += fy;
            forces[index][2] += first_torque;
            forces[target as usize][0] -= fx;
            forces[target as usize][1] -= fy;
            forces[target as usize][2] += second_torque;
            strain_force[index] += difference;
            strain_force[target as usize] -= difference;
        }
    }
    for (index, cell) in world.cells.iter_mut().enumerate() {
        let mass = cell.values[7];
        let radius = cell.values[6];
        if mass > 0.0 && radius > 0.0 {
            cell.values[2] += forces[index][0] * dt / mass;
            cell.values[3] += forces[index][1] * dt / mass;
            cell.values[4] += forces[index][2] * dt / (0.7 * mass * radius * radius);
        }
        cell.q += strain_force[index] * dt * 10.0;
    }
    for (index, target) in broken {
        remove_links_to(&mut world.cells[index], target);
        if target >= 0 {
            remove_links_to(&mut world.cells[target as usize], index as i32);
        }
    }
}

fn spawn_random_cells(world: &mut World) {
    if !world.environment.w || world.cells.len() >= world.environment.max_cells {
        return;
    }
    let allowed: Vec<i32> = world
        .environment
        .enabled_types
        .iter()
        .enumerate()
        .filter_map(|(i, enabled)| (*enabled).then_some(i as i32))
        .collect();
    if allowed.is_empty() {
        return;
    }
    let target = world.environment.max_cells / 10;
    let mut rng = rand::rng();
    let mut count = target.saturating_sub(world.cells.len());
    if rng.random_range(0..15) == 0 {
        count += 1;
    }
    count = count.min(world.environment.max_cells - world.cells.len());
    for _ in 0..count {
        let radius = world.environment.radius;
        let (x, y) = loop {
            let x = rng.random_range(-radius..radius);
            let y = rng.random_range(-radius..radius);
            if x * x + y * y < radius * radius {
                break (x, y);
            }
        };
        world.cells.push(random_cell(x, y, &allowed, &mut rng));
    }
}

fn random_cell(x: f64, y: f64, allowed: &[i32], rng: &mut impl Rng) -> Cell {
    const MODE_COUNTS: [i32; 11] = [40, 40, 4, 6, 15, 4, 4, 4, 4, 20, 6];
    const LOW: [f32; 7] = [-60.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.5];
    const HIGH: [f32; 7] = [60.0, 1.0, 1.0, 1.0, 1.7320508, 0.01, 2.0];
    let mut genes = Vec::with_capacity(40);
    for _ in 0..40 {
        let color = std::array::from_fn(|_| rng.random::<f32>());
        let floats = [
            0.072 + rng.random::<f32>() * 0.288,
            0.5 + rng.random_range(-0.2..0.2),
            rng.random::<f32>() * std::f32::consts::TAU,
            rng.random::<f32>() * std::f32::consts::TAU,
            rng.random::<f32>() * std::f32::consts::TAU,
            10.0 + rng.random_range(-3.0..3.0),
            rng.random::<f32>() * 35.0,
        ];
        let mut inputs = [GeneInput::default(); 11];
        let neural = allowed.iter().any(|kind| matches!(kind, 10 | 11 | 13));
        for input in &mut inputs {
            input.source = rng.random_range(0..8);
            if neural {
                input.mode = rng.random_range(1..=2);
                input.a = rng.random_range(-8.0..8.0);
                input.b = rng.random_range(-8.0..8.0);
                input.c = rng.random_range(-8.0..8.0);
            } else {
                input.mode = 0;
                input.b = rng.random_range(-1.0..1.0);
            }
        }
        let output_modes = std::array::from_fn(|i| rng.random_range(0..MODE_COUNTS[i]));
        let mut output_values =
            std::array::from_fn(|i| LOW[i] + rng.random::<f32>() * (HIGH[i] - LOW[i]));
        output_values[5] = rng.random::<f32>().powi(3) * HIGH[5];
        genes.push(Gene {
            color,
            floats,
            i: rng.random_range(0..40),
            j: rng.random_range(0..40),
            k: 0,
            flags: [
                rng.random::<bool>(),
                rng.random::<bool>(),
                rng.random::<bool>(),
                false,
                false,
                false,
                false,
            ],
            cell_type: allowed[rng.random_range(0..allowed.len())],
            inputs,
            output_modes,
            output_values,
        });
    }
    let mode = rng.random_range(0..40);
    let color = genes[mode as usize].color;
    let lipocyte_reserve = if genes[mode as usize].cell_type == 4 {
        rng.random::<f32>() * 1.8
    } else {
        0.0
    };
    Cell {
        runtime_id: fresh_cell_id(),
        x,
        y,
        values: [
            rng.random_range(-std::f64::consts::PI..std::f64::consts::PI),
            rng.random_range(-std::f64::consts::PI..std::f64::consts::PI),
            0.0,
            0.0,
            0.0,
            0.0,
            (0.216_f64 / 400.0).sqrt(),
            0.216,
            0.0,
        ],
        links: vec![
            Link {
                target: -1,
                c: 0.0,
                d: 0.0,
                b: false,
                e: 0.0,
                f: 0.0,
                k: 0.0,
                l: 0.0
            };
            20
        ],
        genes,
        c: 0,
        x_flag: false,
        color,
        mode,
        e: 2,
        f: 0,
        tuvp: [0.0, 0.0, 0.0, 1.0],
        y_flag: false,
        u: [0.01; 4],
        w: [0.01; 4],
        tail: [0.0, 0.0, 0.0, 0.0, 0.0, lipocyte_reserve],
        g: 0,
        t: -1,
        q: 0.0,
        resting: false,
        visual_phase: 0.0,
        visual_phase2: 0.0,
        mutation_ticks: 0,
    }
}

/// CellWorld.d(int): the circular plate exerts overlap pressure. Glueocytes
/// make fixed plate links where they touch the wall.
fn apply_plate_edge_force(world: &mut World, dt: f64) -> Vec<(usize, Option<usize>)> {
    if world.environment.x {
        return Vec::new();
    }
    let plate_radius = world.environment.radius;
    let mut ciliary_contacts = Vec::new();
    for (index, cell) in world.cells.iter_mut().enumerate() {
        if cell.x_flag {
            continue;
        }
        let distance = cell.x.hypot(cell.y);
        let cell_radius = cell.values[6];
        let mass = cell.values[7];
        if distance < plate_radius - cell_radius
            || distance >= plate_radius
            || distance == 0.0
            || mass <= 0.0
        {
            continue;
        }
        let chord = ((-distance + cell_radius - plate_radius)
            * (-distance - cell_radius + plate_radius)
            * (-distance + cell_radius + plate_radius)
            * (distance + cell_radius + plate_radius))
            / distance;
        let pressure = cell
            .active_gene()
            .map(|gene| gene.output_values[6] as f64)
            .unwrap_or(1.0);
        let force_factor = 1000.0 * chord.max(0.0) * pressure / distance;
        cell.values[2] -= cell.x * force_factor * dt / mass;
        cell.values[3] -= cell.y * force_factor * dt / mass;
        let nx = cell.x / distance;
        let ny = cell.y / distance;
        if cell.cell_type() == 7
            && cell.c < 5
            && !cell
                .links
                .iter()
                .take(cell.c.max(0) as usize)
                .any(|link| link.target == -1)
        {
            add_link(cell, -1, cell.y.atan2(cell.x) - cell.values[0], 0.0);
            let anchor = plate_radius + 0.013_416_407_864_998_738 * 0.4;
            let last = &mut cell.links[cell.c as usize - 1];
            last.e = nx * anchor;
            last.f = ny * anchor;
        }
        if cell.cell_type() == 17 {
            let anchored = cell
                .links
                .iter()
                .take(cell.c.max(0) as usize)
                .any(|link| link.target == -1);
            if !anchored {
                ciliary_contacts.push((index, None));
            }
        }
    }
    ciliary_contacts
}

fn ciliary_slip(cell: &Cell, nx: f64, ny: f64) -> f64 {
    let tangent = cell.values[0].cos() * ny - cell.values[0].sin() * nx;
    if tangent.abs() <= 0.2 {
        return 0.0;
    }
    let output = if (tangent > 0.0) != cell.y_flag { 2 } else { 4 };
    tangent * 0.075 * cell.signal(output) as f64
}

/// CellWorld.h(): resolve the tangential velocity at each ciliary contact
/// after ordinary forces and surface friction have been integrated.
fn apply_ciliary_contacts(world: &mut World, contacts: &mut [(usize, Option<usize>)]) {
    contacts.sort_by_key(|(first, second)| (*first, second.is_none()));
    let plate_radius = world.environment.radius;
    for &(first, second) in contacts.iter() {
        match second {
            Some(second) if first < second && second < world.cells.len() => {
                let (left, right) = world.cells.split_at_mut(second);
                apply_ciliary_pair(&mut left[first], &mut right[0]);
            }
            None if first < world.cells.len() => {
                apply_ciliary_plate(&mut world.cells[first], plate_radius);
            }
            _ => {}
        }
    }
}

fn apply_ciliary_pair(first: &mut Cell, second: &mut Cell) {
    let mass1 = first.values[7];
    let mass2 = second.values[7];
    let radius1 = first.values[6];
    let radius2 = second.values[6];
    let dx = second.x - first.x;
    let dy = second.y - first.y;
    let distance = dx.hypot(dy);
    if mass1 <= 0.0 || mass2 <= 0.0 || radius1 <= 0.0 || radius2 <= 0.0 || distance == 0.0 {
        return;
    }
    let nx = dx / distance;
    let ny = dy / distance;
    let arm1 = (radius1 * radius1 + distance * distance - radius2 * radius2) / (2.0 * distance);
    let arm2 = distance - arm1;
    let inertia1 = 0.7 * mass1 * radius1 * radius1;
    let inertia2 = 0.7 * mass2 * radius2 * radius2;
    let vx1 = first.values[2] - arm1 * ny * first.values[4];
    let vy1 = first.values[3] + arm1 * nx * first.values[4];
    let vx2 = second.values[2] + arm2 * ny * second.values[4];
    let vy2 = second.values[3] - arm2 * nx * second.values[4];
    let relative_x = vx1 - vx2;
    let relative_y = vy1 - vy2;
    let normal = nx * relative_x + ny * relative_y;
    let slip = if first.cell_type() == 17 {
        ciliary_slip(first, nx, ny)
    } else {
        0.0
    } + if second.cell_type() == 17 {
        ciliary_slip(second, -nx, -ny)
    } else {
        0.0
    };
    let tangent_x = relative_x - nx * normal + ny * slip;
    let tangent_y = relative_y - ny * normal - nx * slip;
    // The APK's impulse scale is 0.9 / 0.02, then it applies 0.02 * impulse.
    let effective_mass =
        0.9 / (1.0 / mass1 + 1.0 / mass2 + arm1 * arm1 / inertia1 + arm2 * arm2 / inertia2);
    let impulse_x = -tangent_x * effective_mass;
    let impulse_y = -tangent_y * effective_mass;
    first.values[2] += impulse_x / mass1;
    first.values[3] += impulse_y / mass1;
    second.values[2] -= impulse_x / mass2;
    second.values[3] -= impulse_y / mass2;
    let torque = -impulse_x * ny + impulse_y * nx;
    first.values[4] += arm1 * torque / inertia1;
    second.values[4] += arm2 * torque / inertia2;
}

fn apply_ciliary_plate(cell: &mut Cell, plate_radius: f64) {
    let mass = cell.values[7];
    let radius = cell.values[6];
    let distance = cell.x.hypot(cell.y);
    if mass <= 0.0 || radius <= 0.0 || distance == 0.0 {
        return;
    }
    let nx = cell.x / distance;
    let ny = cell.y / distance;
    let arm = plate_radius - distance;
    let inertia = 0.7 * mass * radius * radius;
    let relative_x = cell.values[2] - arm * ny * cell.values[4];
    let relative_y = cell.values[3] + arm * nx * cell.values[4];
    let normal = nx * relative_x + ny * relative_y;
    let slip = ciliary_slip(cell, nx, ny);
    let tangent_x = relative_x - nx * normal + ny * slip;
    let tangent_y = relative_y - ny * normal - nx * slip;
    let effective_mass = 0.9 / (1.0 / mass + arm * arm / inertia);
    let impulse_x = -tangent_x * effective_mass;
    let impulse_y = -tangent_y * effective_mass;
    cell.values[2] += impulse_x / mass;
    cell.values[3] += impulse_y / mass;
    cell.values[4] += arm * (-impulse_x * ny + impulse_y * nx) / inertia;
}

/// CellWorld.d(int): the squared chord length of the two-circle overlap sets
/// contact pressure. This force is evaluated before infection or feeding can
/// change either cell's mass and is integrated after all pairs are visited.
#[derive(Clone, Copy)]
struct ContactMeta {
    kind: i32,
    resistance: Option<f64>,
    gamete_target: Option<i32>,
}

impl ContactMeta {
    fn from_cell(cell: &Cell) -> Self {
        let gene = cell.active_gene();
        Self {
            kind: gene.map(|gene| gene.cell_type).unwrap_or(0),
            resistance: gene.map(|gene| gene.output_values[6] as f64),
            gamete_target: gene.map(|gene| gene.output_modes[1]),
        }
    }
}

#[cfg(test)]
fn cell_contact_force(first: &Cell, second: &Cell, squared: f64) -> Option<(f64, f64)> {
    cell_contact_force_cached(
        first,
        second,
        squared,
        ContactMeta::from_cell(first),
        ContactMeta::from_cell(second),
    )
}

fn cell_contact_force_cached(
    first: &Cell,
    second: &Cell,
    squared: f64,
    first_meta: ContactMeta,
    second_meta: ContactMeta,
) -> Option<(f64, f64)> {
    let r1 = first.values[6];
    let r2 = second.values[6];
    let outer = r1 + r2;
    let inner = r1 - r2;
    if squared <= inner * inner || squared >= outer * outer {
        return None;
    }
    let resistance1 = first_meta.resistance?;
    let resistance2 = second_meta.resistance?;
    if resistance1 <= 0.0 || resistance2 <= 0.0 {
        return None;
    }
    let fast_distance = squared >= f32::MIN_POSITIVE as f64 && squared <= f32::MAX as f64;
    let inverse_distance = if fast_distance {
        contact_inverse_distance(squared)
    } else {
        1.0 / squared.sqrt()
    };
    let overlap_numerator = (outer * outer - squared) * (squared - inner * inner);
    let mut pressure = if !fast_distance {
        // Preserve the original evaluation order for extremely close centers.
        (overlap_numerator / squared) * (1000.0 * resistance1 * resistance2)
            / ((resistance1 + resistance2) * squared.sqrt())
    } else {
        overlap_numerator
            * (1000.0 * resistance1 * resistance2 / (resistance1 + resistance2))
            * inverse_distance
            * inverse_distance
            * inverse_distance
    };
    if first_meta.kind == 16
        && second_meta.kind == 16
        && first.values[8] > 0.5
        && second.values[8] > 0.5
    {
        let first_target = first_meta.gamete_target;
        let second_target = second_meta.gamete_target;
        if first_target == Some(second.mode) && second_target == Some(first.mode) {
            pressure *= -0.6;
        } else if first_target == Some(second.mode) || second_target == Some(first.mode) {
            pressure = 0.0;
        }
    }
    let mut fx = (first.x - second.x) * pressure;
    let mut fy = (first.y - second.y) * pressure;
    if first_meta.kind == 17 || second_meta.kind == 17 {
        let reduced_mass = first.values[7] * second.values[7]
            / (first.values[7] + second.values[7]).max(f64::MIN_POSITIVE);
        let nx = (second.x - first.x) * inverse_distance;
        let ny = (second.y - first.y) * inverse_distance;
        let relative =
            (second.values[2] - first.values[2]) * nx + (second.values[3] - first.values[3]) * ny;
        let normal_damping = reduced_mass * relative * 0.2 / 0.02;
        fx += nx * normal_damping;
        fy += ny * normal_damping;
    }
    Some((fx, fy))
}

#[inline(always)]
fn contact_inverse_distance(squared: f64) -> f64 {
    #[cfg(target_arch = "x86_64")]
    {
        let mut input = squared as f32;
        // RSQRTSS has about 12 bits of precision. Contact dynamics tolerate
        // its small error, and avoiding the refinement keeps this path short.
        unsafe {
            core::arch::asm!(
                "rsqrtss {value}, {value}",
                value = inout(xmm_reg) input,
                options(pure, nomem, nostack)
            );
        }
        input as f64
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        1.0 / squared.sqrt()
    }
}

fn handle_food_edges(world: &mut World) {
    // CellWorld.d(double) only moves food and handles the edge when y is true.
    // With x false it clamps and reflects food; with x true it removes it.
    if !world.environment.y || world.environment.x {
        return;
    }
    let radius = world.environment.radius as f32;
    for food in &mut world.food {
        let distance = food.x.hypot(food.y);
        if distance >= radius && distance > 0.0 {
            food.x *= radius / distance;
            food.y *= radius / distance;
            let reflection = 2.0 * (food.x * food.vx + food.y * food.vy) / (distance * distance);
            food.vx -= food.x * reflection;
            food.vy -= food.y * reflection;
        }
    }
}

#[cfg(test)]
fn solve_contacts(
    world: &mut World,
    dt: f64,
    preview: bool,
    rng: &mut impl Rng,
) -> Vec<(usize, Option<usize>)> {
    solve_contacts_profile(world, dt, preview, rng, &mut ContactTimings::default())
}

#[derive(Default)]
struct ContactTimings {
    grid: u128,
    search: u128,
    effects: u128,
    marking: u128,
    metadata: u128,
    pairs: u128,
    apply: u128,
}

/// Compact contact bins retain descending cell-index order, matching the
/// original linked-list traversal within each neighboring bin.
struct ContactGrid {
    side: usize,
    offsets: Vec<usize>,
    members: Vec<usize>,
    coords: Vec<(usize, usize)>,
    positions: Vec<(f64, f64, f64)>,
}

impl ContactGrid {
    fn new(cells: &[Cell], radius: f64) -> Self {
        let max_cell_radius = cells
            .iter()
            .map(|cell| cell.values[6])
            .fold(0.0_f64, f64::max);
        let bin = (2.0 * radius / (cells.len() as f64).sqrt())
            .max(2.0 * max_cell_radius)
            .max(0.06);
        let side = ((2.0 * radius / bin).ceil() as usize) + 3;
        let mut offsets = vec![0_usize; side * side + 1];
        let mut coords = Vec::with_capacity(cells.len());
        let mut positions = Vec::with_capacity(cells.len());
        for cell in cells {
            let bx =
                (((cell.x + radius) / bin).floor() as isize).clamp(0, side as isize - 1) as usize;
            let by =
                (((cell.y + radius) / bin).floor() as isize).clamp(0, side as isize - 1) as usize;
            coords.push((bx, by));
            positions.push((cell.x, cell.y, cell.values[6]));
            offsets[by * side + bx + 1] += 1;
        }
        for bucket in 1..offsets.len() {
            offsets[bucket] += offsets[bucket - 1];
        }
        let mut ends = offsets[1..].to_vec();
        let mut members = vec![0; cells.len()];
        for (index, &(bx, by)) in coords.iter().enumerate() {
            let bucket = by * side + bx;
            ends[bucket] -= 1;
            members[ends[bucket]] = index;
        }
        Self {
            side,
            offsets,
            members,
            coords,
            positions,
        }
    }

    fn pairs_in_range(&self, range: std::ops::Range<usize>) -> Vec<(u32, u32, f64)> {
        let mut pairs = Vec::new();
        for i in range {
            let (bx, by) = self.coords[i];
            let (first_x, first_y, first_radius) = self.positions[i];
            for ny in by.saturating_sub(1)..=(by + 1).min(self.side - 1) {
                let row = ny * self.side;
                for nx in bx.saturating_sub(1)..=(bx + 1).min(self.side - 1) {
                    let bucket = row + nx;
                    for &j in &self.members[self.offsets[bucket]..self.offsets[bucket + 1]] {
                        if j <= i {
                            break;
                        }
                        let (second_x, second_y, second_radius) = self.positions[j];
                        let squared = (second_x - first_x).powi(2) + (second_y - first_y).powi(2);
                        let reach = first_radius + second_radius;
                        if squared > 0.0 && squared < reach * reach {
                            pairs.push((i as u32, j as u32, squared));
                        }
                    }
                }
            }
        }
        pairs
    }

    fn overlapping_pairs(&self) -> Vec<(u32, u32, f64)> {
        const CHUNK_CELLS: usize = 512;
        if self.positions.len() < CHUNK_CELLS * 2 {
            return self.pairs_in_range(0..self.positions.len());
        }
        // Rayon collects this indexed range in chunk order. Appending those
        // chunks keeps each pair in the APK's original traversal order.
        let chunks: Vec<Vec<_>> = (0..self.positions.len().div_ceil(CHUNK_CELLS))
            .into_par_iter()
            .map(|chunk| {
                let start = chunk * CHUNK_CELLS;
                self.pairs_in_range(start..(start + CHUNK_CELLS).min(self.positions.len()))
            })
            .collect();
        let mut pairs = Vec::with_capacity(chunks.iter().map(Vec::len).sum());
        for mut chunk in chunks {
            pairs.append(&mut chunk);
        }
        pairs
    }
}

fn solve_contacts_profile(
    world: &mut World,
    dt: f64,
    preview: bool,
    rng: &mut impl Rng,
    timings: &mut ContactTimings,
) -> Vec<(usize, Option<usize>)> {
    let started = Instant::now();
    let cells = &mut world.cells;
    if cells.len() < 2 {
        return Vec::new();
    }
    let radius = world.environment.radius.max(0.05);
    // Any overlapping pair lies in the same or a neighboring bin. Build
    // contiguous bins so the search can scan nearby indices in parallel.
    let grid = ContactGrid::new(cells, radius);
    // Positions and radii do not change during pair effects. Keep the broad
    // phase in a compact array; only overlapping pairs need full Cell access.
    // Two u32 indices keep the cache compact on densely populated plates.
    let grid_built = Instant::now();
    let overlapping_pairs = grid.overlapping_pairs();
    let pairs_found = Instant::now();
    let mut contact_meta: Vec<ContactMeta> = cells.iter().map(ContactMeta::from_cell).collect();
    // CellWorld.i() marks engulfed cells for the entire plate before
    // CellWorld.c() runs contact forces and feeding. Interleaving the two
    // lets a cell feed a neighbor before a later pair marks it for removal.
    for &(first_index, second_index, squared) in &overlapping_pairs {
        let i = first_index as usize;
        let j = second_index as usize;
        let first_radius = grid.positions[i].2;
        let second_radius = grid.positions[j].2;
        let size_difference = first_radius * first_radius - second_radius * second_radius;
        if size_difference > squared {
            cells[j].x_flag = true;
        } else if -size_difference > squared {
            cells[i].x_flag = true;
        }
        // The APK fuses gametes in this same first contact pass, even if
        // size-based engulfment just marked one.
        if contact_meta[i].kind == 16
            && contact_meta[j].kind == 16
            && cells[i].values[8] > 0.5
            && cells[j].values[8] > 0.5
        {
            let (left, right) = cells.split_at_mut(j);
            if gamete_contact(&mut left[i], &mut right[0], rng) {
                contact_meta[i] = ContactMeta::from_cell(&left[i]);
                contact_meta[j] = ContactMeta::from_cell(&right[0]);
            }
        }
    }
    let marked = Instant::now();
    // Fusion may switch the survivor's active mode. The APK observes that
    // new type when it calculates Keratinocyte protection and contact effects.
    let mut has_link_cutters = false;
    for (cell, meta) in cells.iter().zip(&contact_meta) {
        if meta.kind == 14
            && cell.tuvp[3] >= 0.2
            && cell.values[8] > 0.5
            && cell
                .active_gene()
                .is_some_and(|gene| gene.output_modes[4] == 3)
        {
            has_link_cutters = true;
        }
    }
    let keratin_protected: Vec<bool> = cells
        .iter()
        .enumerate()
        .map(|(i, cell)| {
            contact_meta[i].kind == 5
                || cell.links.iter().take(cell.c.max(0) as usize).any(|link| {
                    link.target >= 0
                        && contact_meta
                            .get(link.target as usize)
                            .is_some_and(|meta| meta.kind == 5)
                })
        })
        .collect();
    let mut links_to_cut = Vec::new();
    let mut contact_forces = vec![[0.0_f64; 2]; cells.len()];
    let mut ciliary_contacts = Vec::new();
    let prepared = Instant::now();
    for (first_index, second_index, squared) in overlapping_pairs {
        let i = first_index as usize;
        let j = second_index as usize;
        if !cells[i].x_flag && !cells[j].x_flag {
            let first_meta = contact_meta[i];
            let second_meta = contact_meta[j];
            let (left, right) = cells.split_at_mut(j);
            let first = &mut left[i];
            let second = &mut right[0];
            if let Some((fx, fy)) =
                cell_contact_force_cached(first, second, squared, first_meta, second_meta)
            {
                contact_forces[i][0] += fx;
                contact_forces[i][1] += fy;
                contact_forces[j][0] -= fx;
                contact_forces[j][1] -= fy;
            }
            if first_meta.kind == 17 || second_meta.kind == 17 {
                let linked = first
                    .links
                    .iter()
                    .take(first.c.max(0) as usize)
                    .any(|link| link.target == j as i32)
                    || second
                        .links
                        .iter()
                        .take(second.c.max(0) as usize)
                        .any(|link| link.target == i as i32);
                if !linked {
                    ciliary_contacts.push((i, Some(j)));
                }
            }
            let altered = pair_effects_cached(
                first,
                second,
                i,
                j,
                dt,
                first_meta.kind,
                second_meta.kind,
                keratin_protected[i],
                keratin_protected[j],
                preview,
                rng,
            );
            if has_link_cutters {
                queue_secretion_cuts(first, second, j, &grid.positions, radius, &mut links_to_cut);
                queue_secretion_cuts(second, first, i, &grid.positions, radius, &mut links_to_cut);
            }
            if altered & 1 != 0 {
                contact_meta[i] = ContactMeta::from_cell(first);
            }
            if altered & 2 != 0 {
                contact_meta[j] = ContactMeta::from_cell(second);
            }
        }
    }
    let pairs_done = Instant::now();
    for (cell, force) in cells.iter_mut().zip(contact_forces) {
        if cell.values[7] > 0.0 {
            cell.values[2] += force[0] * dt / cell.values[7];
            cell.values[3] += force[1] * dt / cell.values[7];
        }
    }
    links_to_cut.sort_unstable();
    links_to_cut.dedup();
    for (victim, target) in links_to_cut {
        remove_links_to(&mut cells[victim], target);
        if target >= 0 && (target as usize) < cells.len() {
            remove_links_to(&mut cells[target as usize], victim as i32);
        }
    }
    let effects_done = Instant::now();
    timings.grid = grid_built.duration_since(started).as_nanos();
    timings.search = pairs_found.duration_since(grid_built).as_nanos();
    timings.effects = effects_done.duration_since(pairs_found).as_nanos();
    timings.marking = marked.duration_since(pairs_found).as_nanos();
    timings.metadata = prepared.duration_since(marked).as_nanos();
    timings.pairs = pairs_done.duration_since(prepared).as_nanos();
    timings.apply = effects_done.duration_since(pairs_done).as_nanos();
    ciliary_contacts
}

fn queue_secretion_cuts(
    source: &Cell,
    victim: &Cell,
    victim_index: usize,
    positions: &[(f64, f64, f64)],
    plate_radius: f64,
    cuts: &mut Vec<(usize, i32)>,
) {
    if source.x_flag
        || victim.x_flag
        || source.cell_type() != 14
        || source.tuvp[3] < 0.2
        || source.values[8] <= 0.5
        || !source
            .active_gene()
            .is_some_and(|gene| gene.output_modes[4] == 3)
    {
        return;
    }
    let at_edge = source.x.hypot(source.y) >= plate_radius - source.values[6];
    for link in victim.links.iter().take(victim.c.max(0) as usize) {
        if link.target == -1 {
            if at_edge {
                cuts.push((victim_index, -1));
            }
        } else if link.target >= 0 {
            if let Some(&(x, y, other_radius)) = positions.get(link.target as usize) {
                let reach = source.values[6] + other_radius;
                if (source.x - x).powi(2) + (source.y - y).powi(2) <= reach * reach {
                    cuts.push((victim_index, link.target));
                }
            }
        }
    }
}

fn remove_links_to(cell: &mut Cell, target: i32) {
    let mut active = (cell.c.max(0) as usize).min(cell.links.len());
    let mut slot = 0;
    while slot < active {
        if cell.links[slot].target == target {
            active -= 1;
            cell.links.swap(slot, active);
        } else {
            slot += 1;
        }
    }
    cell.c = active as i32;
}

#[cfg(test)]
fn pair_effects(
    first: &mut Cell,
    second: &mut Cell,
    i: usize,
    j: usize,
    dt: f64,
    first_protected: bool,
    second_protected: bool,
    preview: bool,
    rng: &mut impl Rng,
) -> u8 {
    let first_type = first.cell_type();
    let second_type = second.cell_type();
    pair_effects_cached(
        first,
        second,
        i,
        j,
        dt,
        first_type,
        second_type,
        first_protected,
        second_protected,
        preview,
        rng,
    )
}

fn pair_effects_cached(
    first: &mut Cell,
    second: &mut Cell,
    i: usize,
    j: usize,
    dt: f64,
    first_type: i32,
    second_type: i32,
    first_protected: bool,
    second_protected: bool,
    preview: bool,
    rng: &mut impl Rng,
) -> u8 {
    if first_type == 16 && second_type == 16 && first.values[8] > 0.5 && second.values[8] > 0.5 {
        return 0;
    }
    if first.x_flag || second.x_flag {
        return 0;
    }
    let mut altered = 0;
    if first_type == 3 && !second_protected {
        devour(first, second, dt);
    }
    if second_type == 3 && !first_protected {
        devour(second, first, dt);
    }
    if !preview && first_type == 8 && !second_protected && rng.random::<f64>() < (dt * 5.0).min(1.0)
    {
        infect(first, second);
        altered |= 2;
    }
    if !preview && second_type == 8 && !first_protected && rng.random::<f64>() < (dt * 5.0).min(1.0)
    {
        infect(second, first);
        altered |= 1;
    }
    if first_type == 14 {
        secrete(first, second, second_type, second_protected, dt);
    }
    if second_type == 14 {
        secrete(second, first, first_type, first_protected, dt);
    }
    let glue = (first_type == 7 && !second_protected) || (second_type == 7 && !first_protected);
    if glue
        && first.values[8] >= 0.1
        && second.values[8] >= 0.1
        && first.c < 20
        && second.c < 20
        && ((first_type == 7 && first.c < 5) || (second_type == 7 && second.c < 5))
        && !first
            .links
            .iter()
            .take(first.c.max(0) as usize)
            .any(|link| link.target == j as i32)
        && !second
            .links
            .iter()
            .take(second.c.max(0) as usize)
            .any(|link| link.target == i as i32)
    {
        let angle = (second.y - first.y).atan2(second.x - first.x);
        add_link(
            first,
            j as i32,
            angle - first.values[0],
            angle + std::f64::consts::PI - second.values[0],
        );
        add_link(
            second,
            i as i32,
            angle + std::f64::consts::PI - second.values[0],
            angle - first.values[0],
        );
    }
    altered
}

fn gamete_contact(first: &mut Cell, second: &mut Cell, rng: &mut impl Rng) -> bool {
    let dx = second.x - first.x;
    let dy = second.y - first.y;
    let squared = dx * dx + dy * dy;
    let difference = first.values[6] * first.values[6] - second.values[6] * second.values[6];
    let tolerance = 1.8e-6; // Cell.a * Cell.a * .01 in CellWorld.i().
    let first_can_absorb = difference > squared - tolerance;
    let second_can_absorb = -difference > squared - tolerance;
    if !first_can_absorb && !second_can_absorb {
        return false;
    }
    // CellWorld.i() evaluates both branches in order. When radii are very
    // close, its second branch can replace the first branch's random choice.
    let mut first_survives = if first_can_absorb {
        difference > squared || rng.random::<bool>()
    } else {
        false
    };
    if second_can_absorb {
        first_survives = !(-difference > squared || rng.random::<bool>());
    }
    if first_survives {
        fuse_gametes(first, second, rng);
    } else {
        fuse_gametes(second, first, rng);
    }
    true
}

fn fuse_gametes(survivor: &mut Cell, donor: &mut Cell, rng: &mut impl Rng) {
    donor.x_flag = true;
    survivor.values[7] = (survivor.values[7] + donor.values[7]).min(0.36);
    donor.values[7] = 0.0;
    survivor.values[8] = 0.0;
    survivor.tuvp[3] = (survivor.tuvp[3] + donor.tuvp[3]).min(1.0);
    if survivor.e != donor.e {
        survivor.e = 6;
    }
    if let Some((stage, next)) = survivor
        .active_gene()
        .map(|gene| (gene.output_modes[10], gene.output_modes[0]))
    {
        let stage = stage.clamp(0, 5) as usize;
        survivor.t = [-1, 5, 10, 20, 40, 80][stage];
        survivor.mode = next.clamp(0, survivor.genes.len().saturating_sub(1) as i32);
    }
    for (target, source) in survivor.genes.iter_mut().zip(&donor.genes) {
        if rng.random::<bool>() {
            *target = source.clone();
        }
    }
    if let Some(gene) = survivor.active_gene() {
        survivor.color = gene.color;
    }
}

fn infect(source: &Cell, victim: &mut Cell) {
    victim.e = match victim.e {
        0 => 3,
        1 => 4,
        2 => 5,
        other => other,
    };
    let Some(gene) = source.active_gene() else {
        return;
    };
    let root = gene.output_modes[0].rem_euclid(40) as usize;
    let Some(root_gene) = source.genes.get(root) else {
        return;
    };
    let child_a = root_gene.i.rem_euclid(40) as usize;
    let child_b = root_gene.j.rem_euclid(40) as usize;
    let mut indexes = vec![root, child_a, child_b];
    for child in [child_a, child_b] {
        if let Some(gene) = source.genes.get(child) {
            indexes.push(gene.i.rem_euclid(40) as usize);
            indexes.push(gene.j.rem_euclid(40) as usize);
        }
    }
    for index in indexes {
        if let (Some(donor), Some(receiver)) =
            (source.genes.get(index), victim.genes.get_mut(index))
        {
            *receiver = donor.clone();
        }
    }
}

fn devour(attacker: &mut Cell, victim: &mut Cell, dt: f64) {
    let amount = (0.72 * dt).min(victim.values[7] - 0.036).max(0.0);
    attacker.values[7] += amount;
    victim.values[7] -= amount;
    if amount > 0.0 {
        // CellWorld.e(): nutrients and toxin move with the stolen mass.
        let fraction = amount / victim.values[7].max(0.036);
        let nutrient = victim.tuvp[3] * fraction;
        attacker.tuvp[3] += nutrient;
        victim.tuvp[3] -= nutrient;
        let toxin = victim.tail[0] as f64 * fraction;
        attacker.tail[0] += toxin as f32;
        victim.tail[0] -= toxin as f32;
    }
}

fn secrete(source: &Cell, victim: &mut Cell, victim_type: i32, protected: bool, dt: f64) {
    if source.tuvp[3] < 0.2 {
        return;
    }
    let mode = source
        .active_gene()
        .map(|gene| gene.output_modes[4])
        .unwrap_or(0);
    match mode {
        1 if !protected
            && (victim_type != 14
                || victim
                    .active_gene()
                    .is_none_or(|gene| gene.output_modes[4] != 1)) =>
        {
            victim.tail[0] += (60.0 * dt) as f32;
        }
        2 if victim_type != 5
            && (victim_type != 14
                || victim
                    .active_gene()
                    .is_none_or(|gene| gene.output_modes[4] != 2)) =>
        {
            victim.tail[1] += (0.1 * dt) as f32;
        }
        7..=10 if victim_type != 4 => victim.u[(mode - 7) as usize] -= (60.0 * dt) as f32,
        11..=14 if victim_type != 4 => victim.u[(mode - 11) as usize] += (60.0 * dt) as f32,
        _ => {}
    }
}

fn add_link(cell: &mut Cell, target: i32, c: f64, d: f64) {
    let index = cell.c.max(0) as usize;
    let link = Link {
        target,
        c,
        d,
        b: false,
        e: 0.0,
        f: 0.0,
        k: 0.5,
        l: 0.0,
    };
    if index < cell.links.len() {
        cell.links[index] = link;
    } else {
        cell.links.push(link);
    }
    cell.c += 1;
}

fn compact_cells(world: &mut World, mut keep: impl FnMut(usize, &Cell) -> bool) {
    compact_cells_with_recycling(world, &mut keep, false);
}

fn compact_cells_with_recycling(
    world: &mut World,
    mut keep: impl FnMut(usize, &Cell) -> bool,
    recycle: bool,
) {
    let old_len = world.cells.len();
    // Most steps remove nobody. Allocate the index map only after the first
    // removal; entries before it map to themselves.
    let mut map: Option<Vec<Option<usize>>> = None;
    let dead_limit =
        world.environment.max_cells / 4 + usize::from(world.environment.max_cells % 4 != 0);
    let plate_radius_squared = world.environment.radius * world.environment.radius;
    let food_capacity = world.environment.max_food.saturating_sub(world.food.len());
    let mut recycled_food = Vec::new();
    let dead = &mut world.dead;
    let mut old = 0;
    let mut new = 0;
    world.cells.retain(|cell| {
        let kept = keep(old, cell);
        if kept {
            if let Some(map) = &mut map {
                map[old] = Some(new);
            }
            new += 1;
        } else {
            if map.is_none() {
                let mut indexes = vec![None; old_len];
                for (index, entry) in indexes.iter_mut().take(old).enumerate() {
                    *entry = Some(index);
                }
                map = Some(indexes);
            }
            if cell.x * cell.x + cell.y * cell.y <= plate_radius_squared {
                // CellWorld.j() recycles natural deaths inside the plate, while
                // the Remove tool and forced cap eviction call a(i, false).
                if recycle && cell.values[7] > 0.0 && recycled_food.len() < food_capacity {
                    recycled_food.push(Food {
                        x: cell.x as f32,
                        y: cell.y as f32,
                        energy: (0.9 * cell.values[7]) as f32,
                        vx: 0.0,
                        vy: 0.0,
                        state: if cell.cell_type() == 4 { 0.5 } else { 0.0 },
                    });
                }
                if dead.len() < dead_limit {
                    dead.push(DeadCell {
                        x: cell.x as f32,
                        y: cell.y as f32,
                        radius: cell.values[6] as f32,
                        age: 0.0,
                        color: cell.color,
                    });
                }
            }
        }
        old += 1;
        kept
    });
    world.food.extend(recycled_food);
    if new == old {
        return;
    }
    let map = map.expect("a removed cell creates an index map");
    for cell in &mut world.cells {
        remap_active_links(cell, |target| {
            map.get(target as usize)
                .and_then(|index| *index)
                .map(|index| index as i32)
        });
    }
}

/// Compact only the active prefix. Inactive slots retain their order and the
/// link allocation stays with its cell, including across repeated deaths.
fn remap_active_links(cell: &mut Cell, mut remap: impl FnMut(i32) -> Option<i32>) {
    let active = (cell.c.max(0) as usize).min(cell.links.len());
    let mut kept = 0;
    for slot in 0..active {
        let target = cell.links[slot].target;
        let mapped = if target >= 0 {
            remap(target)
        } else {
            Some(target)
        };
        if let Some(target) = mapped {
            cell.links[slot].target = target;
            cell.links.swap(kept, slot);
            kept += 1;
        }
    }
    cell.c = kept as i32;
    cell.links.drain(kept..active);
}

fn evict_cell_for_placement(world: &mut World, victim: usize) {
    let last = world.cells.len() - 1;
    world.cells.swap_remove(victim);
    for cell in &mut world.cells {
        remap_active_links(cell, |target| {
            if target as usize == victim {
                None
            } else if target as usize == last {
                Some(victim as i32)
            } else {
                Some(target)
            }
        });
    }
}

pub(crate) fn light_at(
    x: f64,
    y: f64,
    radius: f64,
    angle: f64,
    intensity: f64,
    diffusion: f64,
) -> f64 {
    let diffuse = diffusion.clamp(0.01, 1.0);
    let facing = (angle.cos() * x + angle.sin() * y) / radius.max(0.05);
    let radial = x.hypot(y) / radius.max(0.05);
    let falloff = 1.0 + (1.0 - diffuse) * (1.0 - radial) / diffuse;
    (((facing * (1.0 - diffuse) + diffuse) * intensity) / (falloff * falloff)).max(0.0)
}

/// CellWorld.d/e(double,double): directional sensors use the gradient of the
/// plate's light field, including its radial falloff.
pub(crate) fn light_gradient(
    x: f64,
    y: f64,
    radius: f64,
    angle: f64,
    intensity: f64,
    diffusion: f64,
) -> (f64, f64) {
    let radius = radius.max(0.05);
    let diffuse = diffusion.clamp(0.01, 1.0);
    let (sin, cos) = angle.sin_cos();
    let distance = x.hypot(y);
    let light_factor = diffuse + (1.0 - diffuse) * (cos * x + sin * y) / radius;
    if intensity * light_factor <= 0.0 {
        return (0.0, 0.0);
    }
    let falloff = 1.0 + (1.0 - diffuse) * (1.0 - distance / radius) / diffuse;
    if falloff == 0.0 {
        return (0.0, 0.0);
    }
    let scale = intensity * (1.0 - diffuse) / (radius * falloff * falloff);
    let radial = if distance > 0.0 {
        2.0 * light_factor / (diffuse * falloff * distance)
    } else {
        0.0
    };
    (scale * (cos + radial * x), scale * (sin + radial * y))
}

struct ResourceState {
    mass: f64,
    effective_mass: f64,
    transport: f64,
    nutrient: f64,
    toxin: f64,
}

fn capture_resource_states(world: &World) -> Vec<ResourceState> {
    world
        .cells
        .iter()
        .map(|cell| ResourceState {
            mass: cell.values[7],
            effective_mass: 0.0,
            transport: 0.0,
            nutrient: cell.tuvp[3],
            toxin: cell.tail[0] as f64,
        })
        .collect()
}

/// CellWorld.g(double): growth and nutrient replenishment are staged into Q/R;
/// link transfer, starvation, and transport use the original w/p/X values.
fn exchange_link_resources(world: &mut World, dt: f64, mut states: Vec<ResourceState>) {
    for (state, cell) in states.iter_mut().zip(&world.cells) {
        state.effective_mass = if cell.cell_type() == 4 {
            (state.mass + cell.tail[5] as f64) / 6.0
        } else {
            state.mass
        };
        state.transport = cell
            .active_gene()
            .map(|gene| gene.floats[5] as f64)
            .unwrap_or(0.0);
        if let Some(gene) = cell.active_gene() {
            if gene.floats[0] > 0.36 && state.effective_mass > 0.3456 {
                state.transport = 0.0;
            }
            if gene.flags[3] && state.mass < 0.0936 {
                state.transport += 462.962_962_962_962_76 * (0.0936 - state.mass);
            }
        }
    }
    let ambient_nutrient = world.environment.z;
    world
        .cells
        .par_iter_mut()
        .enumerate()
        .for_each(|(index, cell)| {
            let old = &states[index];
            let mut delta = [0.0; 3];
            for link in cell.links.iter().take(cell.c.max(0) as usize) {
                let Some(other) = (link.target >= 0)
                    .then(|| states.get(link.target as usize))
                    .flatten()
                else {
                    continue;
                };
                let mass_flow = (other.transport * old.effective_mass
                    - other.effective_mass * old.transport)
                    .clamp(-0.1, 0.1);
                delta[0] -= mass_flow * dt;
                delta[1] -= (old.nutrient - other.nutrient).clamp(-0.6, 0.6) * dt;
                delta[2] += (other.toxin - old.toxin) * 3.0 * dt;
            }
            cell.tail[1] += ((old.toxin.min(10.0) - cell.tail[1] as f64) * 0.05 * dt) as f32;
            cell.tail[0] = (old.toxin * (1.0 - 0.05 * dt) + delta[2]) as f32;
            cell.values[7] += delta[0];
            let mut nutrient = old.nutrient;
            if nutrient < ambient_nutrient {
                nutrient += 1.2 * (ambient_nutrient - nutrient) * dt;
            }
            match cell.cell_type() {
                9 => nutrient = 1.0,
                14 => {
                    if nutrient > 0.25 {
                        nutrient = (nutrient - 0.45 * dt).max(0.25);
                    }
                    if nutrient >= 0.2 {
                        cell.visual_phase += dt as f32;
                    }
                }
                _ => {}
            }
            cell.tuvp[3] = (nutrient + delta[1]).min(1.0);
            if old.nutrient < 0.1 {
                cell.x_flag = true;
            }
            if cell.cell_type() == 4 {
                if cell.values[7] > 0.36 {
                    cell.tail[5] = (cell.tail[5] + (cell.values[7] - 0.36) as f32).min(1.8);
                    cell.values[7] = 0.36;
                } else if cell.tail[5] > 0.0 {
                    let refill = (0.36 - cell.values[7]).min(cell.tail[5] as f64);
                    cell.values[7] += refill;
                    cell.tail[5] -= refill as f32;
                }
            } else {
                cell.values[7] = cell.values[7].min(0.36);
            }
        });
}

fn reproduce(world: &mut World) {
    if world.cells.len() >= world.environment.max_cells {
        return;
    }
    let original_count = world.cells.len();
    let mut children = Vec::new();
    let mut divided = vec![false; original_count];
    struct InheritedLink {
        old_source: usize,
        old_neighbor: usize,
        new_source: usize,
        slot: usize,
        reciprocal: Link,
    }
    let mut inherited_links: Vec<InheritedLink> = Vec::new();
    for (parent_index, parent) in world.cells.iter_mut().take(original_count).enumerate() {
        if original_count + children.len() >= world.environment.max_cells {
            break;
        }
        let Some(gene) = parent.active_gene().cloned() else {
            continue;
        };
        if gene.cell_type == 16
            || parent.values[8] <= 0.5
            || parent.tuvp[3] <= 0.2
            || parent.x_flag
            || parent.c > gene.output_modes[9]
        {
            continue;
        }
        let effective_mass = if gene.cell_type == 4 {
            0.072 + ((parent.values[7] + parent.tail[5] as f64 - 0.072) / 2.088) * 0.288
        } else {
            parent.values[7]
        };
        let threshold = gene.floats[0] as f64;
        if !threshold.is_finite() || effective_mass <= threshold {
            continue;
        }
        let split = gene.floats[1] as f64;
        if !split.is_finite() || !(0.0..=1.0).contains(&split) {
            continue;
        }
        let handedness = if parent.y_flag { -1.0 } else { 1.0 };
        let old_angle = parent.values[0];
        let old_links: Vec<Link> = parent
            .links
            .iter()
            .take(parent.c.max(0) as usize)
            .filter(|link| link.target >= 0 && (link.target as usize) < original_count)
            .cloned()
            .collect();
        let angle = parent.values[0] + handedness * gene.floats[2] as f64;
        let offset = parent.values[6] * 0.1;
        let left_mode = gene.i.max(0) as usize;
        let right_mode = gene.j.max(0) as usize;
        if left_mode >= parent.genes.len() || right_mode >= parent.genes.len() {
            continue;
        }
        let combined_mass = parent.values[7]
            + if gene.cell_type == 4 {
                parent.tail[5] as f64
            } else {
                0.0
            };
        let link_signal = parent.signal(5);
        // CellWorld.e(double) subtracts h.v (zero for every APK cell type),
        // then only the extra armor cost above output value 1.0.
        let left_penalty =
            ((parent.genes[left_mode].output_values[6] - 1.0).max(0.0) * 0.054000005) as f64;
        let right_penalty =
            ((parent.genes[right_mode].output_values[6] - 1.0).max(0.0) * 0.054000005) as f64;
        let mut child = parent.clone();
        child.runtime_id = fresh_cell_id();
        parent.mutation_ticks = 0;
        child.mutation_ticks = 0;
        child.f = 0;
        // CellWorld.e() perturbs each daughter by a deterministic ~1e-7
        // offset. It breaks exact overlap in large colonies with identical
        // division angles, which otherwise changes subsequent contacts.
        let division_index = (original_count + children.len() + 1) as f64;
        child.x += angle.cos() * offset + (division_index * 12.34 + 13.4).sin() * 1.0e-7;
        child.y += angle.sin() * offset + (division_index * 19.34 + 63.0).sin() * 1.0e-7;
        child.values[7] = combined_mass * split - right_penalty;
        child.values[8] = 0.0;
        child.values[0] = angle + handedness * gene.floats[4] as f64;
        child.values[1] -= 1.0;
        child.mode = right_mode as i32;
        child.color = child
            .genes
            .get(child.mode as usize)
            .map(|g| g.color)
            .unwrap_or(child.color);
        child.links.clear();
        child.c = 0;
        child.x_flag = false;
        child.y_flag ^= gene.flags[6];
        parent.x -= angle.cos() * offset + (division_index * 10.34 + 43.0).sin() * 1.0e-7;
        parent.y -= angle.sin() * offset + (division_index * 17.34 + 137.0).sin() * 1.0e-7;
        parent.values[7] = combined_mass * (1.0 - split) - left_penalty;
        parent.values[8] = 0.0;
        parent.values[0] = angle + handedness * gene.floats[3] as f64;
        parent.values[1] += 1.0;
        parent.mode = left_mode as i32;
        parent.color = parent
            .genes
            .get(parent.mode as usize)
            .map(|g| g.color)
            .unwrap_or(parent.color);
        parent.links.clear();
        parent.c = 0;
        parent.x_flag = false;
        parent.y_flag ^= gene.flags[5];
        for channel in 0..4 {
            let signal = parent.u[channel];
            parent.u[channel] = signal * split as f32 + 0.0001;
            child.u[channel] = signal * (1.0 - split) as f32 + 0.0001;
        }
        for channel in 0..2 {
            let signal = parent.tail[channel];
            parent.tail[channel] = signal * split as f32;
            child.tail[channel] = signal * (1.0 - split) as f32;
        }
        parent.tail[5] = 0.0;
        child.tail[5] = 0.0;
        parent.tuvp[3] *= 0.5;
        child.tuvp[3] = parent.tuvp[3];
        let child_index = original_count + children.len();
        let max_inherited = 20 - usize::from(gene.flags[0]);
        for mut link in old_links {
            let neighbor = link.target as usize;
            if neighbor == parent_index {
                continue;
            }
            let facing = (old_angle + link.c - angle).cos() - (1.0 - 2.0 * split);
            let (target, recipient) = if facing > 0.0 && gene.flags[2] {
                (child_index, &mut child)
            } else if facing <= 0.0 && gene.flags[1] {
                (parent_index, &mut *parent)
            } else {
                continue;
            };
            if recipient.links.len() >= max_inherited {
                continue;
            }
            link.c += old_angle - recipient.values[0];
            let mut reciprocal = link.clone();
            reciprocal.target = target as i32;
            reciprocal.c = link.d;
            reciprocal.d = link.c;
            let slot = recipient.links.len();
            recipient.links.push(link);
            inherited_links.push(InheritedLink {
                old_source: parent_index,
                old_neighbor: neighbor,
                new_source: target,
                slot,
                reciprocal,
            });
        }
        if gene.flags[0] {
            let parent_link = Link {
                target: child_index as i32,
                c: angle - parent.values[0],
                d: std::f64::consts::PI + angle - child.values[0],
                b: true,
                e: 0.0,
                f: 0.0,
                k: gene.floats[6],
                l: link_signal,
            };
            let mut child_link = parent_link.clone();
            child_link.target = parent_index as i32;
            child_link.c = parent_link.d;
            child_link.d = parent_link.c;
            parent.links.push(parent_link);
            child.links.push(child_link);
        }
        parent.c = parent.links.len() as i32;
        child.c = child.links.len() as i32;
        for cell in [&mut *parent, &mut child] {
            if cell.values[7] > 0.36 {
                if cell.cell_type() == 4 {
                    cell.tail[5] = (cell.values[7] - 0.36) as f32;
                }
                cell.values[7] = 0.36;
            }
            if cell.cell_type() == 1
                && cell
                    .links
                    .iter()
                    .take(cell.c.max(0) as usize)
                    .any(|link| link.c.cos() < -0.8)
            {
                cell.x_flag = true;
            }
        }
        divided[parent_index] = true;
        children.push(child);
    }
    world.cells.extend(children);
    if divided.iter().any(|&yes| yes) {
        for (index, cell) in world.cells.iter_mut().take(original_count).enumerate() {
            if divided[index] {
                continue;
            }
            cell.links.truncate(cell.c.max(0) as usize);
            cell.links.retain(|link| {
                link.target >= 0 && !divided.get(link.target as usize).copied().unwrap_or(false)
            });
            cell.c = cell.links.len() as i32;
        }
    }
    let by_old_edge: std::collections::HashMap<(usize, usize), usize> = inherited_links
        .iter()
        .enumerate()
        .map(|(index, link)| ((link.old_source, link.old_neighbor), index))
        .collect();
    let mut dropped = vec![Vec::new(); world.cells.len()];
    for link in &inherited_links {
        if divided[link.old_neighbor] {
            if let Some(other) = by_old_edge.get(&(link.old_neighbor, link.old_source)) {
                let other = &inherited_links[*other];
                let outgoing = &mut world.cells[link.new_source].links[link.slot];
                outgoing.target = other.new_source as i32;
                outgoing.d = other.reciprocal.d;
            } else {
                dropped[link.new_source].push(link.slot);
            }
            continue;
        }
        let cell = &mut world.cells[link.old_neighbor];
        if cell.links.len() < 20 {
            cell.links.push(link.reciprocal.clone());
            cell.c = cell.links.len() as i32;
        } else {
            dropped[link.new_source].push(link.slot);
        }
    }
    for (cell, slots) in world.cells.iter_mut().zip(dropped) {
        if !slots.is_empty() {
            let mut index = 0;
            cell.links.retain(|_| {
                let keep = !slots.contains(&index);
                index += 1;
                keep
            });
            cell.c = cell.links.len() as i32;
        }
    }
}

fn spawn_food(world: &mut World, dt: f64) {
    if world.food.len() >= world.environment.max_food {
        return;
    }
    let env = &world.environment;
    let expectation =
        (env.a * env.radius * env.radius / env.b.max(0.0001) / (1.0 + env.p.max(0.0)) * dt)
            .clamp(0.0, 10_000.0);
    let mut rng = rand::rng();
    for _ in 0..poisson(&mut rng, expectation) {
        if world.food.len() >= env.max_food {
            break;
        }
        let (x, y) = loop {
            let x = rng.random_range(-env.radius..env.radius);
            let y = rng.random_range(-env.radius..env.radius);
            if x * x + y * y <= env.radius * env.radius {
                break (x, y);
            }
        };
        let cluster = if env.p > 0.0 {
            poisson(&mut rng, 1.0 + env.p)
        } else {
            1
        };
        for _ in 0..cluster {
            if world.food.len() >= env.max_food {
                break;
            }
            let (gx, gy) = gaussian_pair(&mut rng);
            let fx = x + gx * env.q;
            let fy = y + gy * env.q;
            if fx * fx + fy * fy < env.radius * env.radius {
                world.food.push(Food {
                    x: fx as f32,
                    y: fy as f32,
                    energy: env.b as f32,
                    vx: 0.0,
                    vy: 0.0,
                    state: env.o,
                });
            }
        }
    }
}

fn poisson(rng: &mut impl Rng, expectation: f64) -> usize {
    let mut remaining = expectation.clamp(0.0, 10_000.0);
    let mut count = 0;
    // Poisson variables add. Chunks avoid exp(-lambda) underflow on rich plates.
    while remaining > 0.0 {
        let part = remaining.min(24.0);
        let threshold = (-part).exp();
        let mut product = 1.0;
        loop {
            product *= rng.random::<f64>();
            if product <= threshold {
                break;
            }
            count += 1;
        }
        remaining -= part;
    }
    count
}

fn gaussian_pair(rng: &mut impl Rng) -> (f64, f64) {
    let radius = (-2.0 * (1.0 - rng.random::<f64>()).ln()).sqrt();
    let angle = std::f64::consts::TAU * rng.random::<f64>();
    (radius * angle.cos(), radius * angle.sin())
}

fn eat_food(world: &mut World, dt: f64) {
    // CellWorld.d(): handle food contacts before integrating moving food.
    // Linked bins avoid per-cell hash allocations at high population counts.
    let radius = world.environment.radius.max(0.05);
    // The food check reaches one food radius beyond the largest cell. Keep
    // that whole reach in the same or a neighboring bin on resized plates.
    let max_reach = world
        .cells
        .iter()
        .map(|cell| cell.values[6] + 0.007)
        .fold(0.0_f64, f64::max);
    let bin = (2.0 * radius / (world.cells.len().max(1) as f64).sqrt())
        .max(max_reach)
        .max(0.05);
    let side = ((2.0 * radius / bin).ceil() as usize) + 3;
    let mut heads = vec![usize::MAX; side * side];
    let mut next = vec![usize::MAX; world.cells.len()];
    let coordinate = |value: f64| {
        (((value + radius) / bin).floor() as isize).clamp(0, side as isize - 1) as usize
    };
    for index in (0..world.cells.len()).rev() {
        let cell = &world.cells[index];
        let slot = coordinate(cell.y) * side + coordinate(cell.x);
        next[index] = heads[slot];
        heads[slot] = index;
    }
    let moving = world.environment.y;
    let damping = 1.0 - 0.2 * world.environment.g * dt;
    for food in &mut world.food {
        if food.energy <= 0.0 {
            continue;
        }
        let x = food.x as f64;
        let y = food.y as f64;
        let gx = coordinate(x) as isize;
        let gy = coordinate(y) as isize;
        let mut vx = food.vx as f64;
        let mut vy = food.vy as f64;
        'nearby: for dx in -1..=1 {
            for dy in -1..=1 {
                let bx = gx + dx;
                let by = gy + dy;
                if bx < 0 || by < 0 || bx >= side as isize || by >= side as isize {
                    continue;
                }
                let mut index = heads[by as usize * side + bx as usize];
                while index != usize::MAX {
                    let current = index;
                    index = next[current];
                    let cell = &mut world.cells[current];
                    let ox = x - cell.x;
                    let oy = y - cell.y;
                    let squared = ox * ox + oy * oy;
                    let contact = cell.values[6] + 0.007;
                    if squared >= contact * contact {
                        continue;
                    }
                    let kind = cell.cell_type();
                    if moving && squared > 1.0e-18 {
                        let distance = squared.sqrt();
                        let nx = ox / distance;
                        let ny = oy / distance;
                        if kind == 17 {
                            let tangent = cell.values[0].cos() * ny - cell.values[0].sin() * nx;
                            let output = if (tangent > 0.0) != cell.y_flag { 2 } else { 4 };
                            let slip = tangent * 0.075 * cell.signal(output) as f64;
                            let tangential_force = (slip - (food.vx as f64 - cell.values[2]) * ny
                                + (food.vy as f64 - cell.values[3]) * nx
                                - cell.values[4] * cell.values[6])
                                * 40.0;
                            vx += (-0.12 * nx + ny * tangential_force) * dt;
                            vy += (-0.12 * ny - nx * tangential_force) * dt;
                        }
                        if distance < cell.values[6] {
                            let force = 1500.0 * (cell.values[6] - distance) / distance;
                            vx += ox * force * dt;
                            vy += oy * force * dt;
                        }
                    }
                    if kind == 14
                        && cell
                            .active_gene()
                            .is_some_and(|gene| gene.output_modes[4] == 2)
                    {
                        food.state -= dt as f32;
                    }
                    if kind == 0 && food.state <= 0.0 {
                        let amount = dt
                            .min(food.energy as f64)
                            .min((0.36 - cell.values[7]).max(0.0));
                        if amount > 0.0 {
                            let old_mass = cell.values[7];
                            let new_mass = old_mass + amount;
                            cell.values[2] *= old_mass / new_mass;
                            cell.values[3] *= old_mass / new_mass;
                            cell.values[7] = new_mass;
                            food.energy -= amount as f32;
                            if food.energy <= 0.0 {
                                break 'nearby;
                            }
                        }
                    }
                }
            }
        }
        if moving && food.energy > 0.0 {
            vx *= damping;
            vy *= damping;
            food.vx = vx as f32;
            food.vy = vy as f32;
            food.x += (vx * dt) as f32;
            food.y += (vy * dt) as f32;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assembly_inverse_distance_stays_close_across_contact_scales() {
        for squared in [1.0e-20_f64, 1.0e-10, 0.0001, 0.0036] {
            let exact = 1.0 / squared.sqrt();
            let actual = contact_inverse_distance(squared);
            assert!((actual / exact - 1.0).abs() < 5.0e-4, "squared={squared}");
        }
    }

    #[test]
    fn parallel_contact_grid_finds_every_overlap_in_serial_order() {
        let world = legacy::import(bundled::PLATES[0].1).unwrap();
        let template = &world.cells[0];
        let mut cells = Vec::with_capacity(1536);
        for index in 0..1536 {
            let mut cell = template.clone();
            cell.x = ((index * 997) % 1536) as f64 / 768.0 - 1.0;
            cell.y = ((index * 787) % 1536) as f64 / 768.0 - 1.0;
            cell.values[6] = if index % 53 == 0 {
                0.08
            } else {
                0.0135 + (index % 4) as f64 * 0.005
            };
            cells.push(cell);
        }
        // Coincident centers must still be excluded from cell contact.
        cells[1].x = cells[0].x;
        cells[1].y = cells[0].y;
        let grid = ContactGrid::new(&cells, 1.0);
        let parallel = grid.overlapping_pairs();
        assert_eq!(parallel, grid.pairs_in_range(0..cells.len()));
        let mut actual: Vec<_> = parallel.iter().map(|&(i, j, _)| (i, j)).collect();
        actual.sort_unstable();
        let mut expected = Vec::new();
        for i in 0..cells.len() {
            for j in i + 1..cells.len() {
                let squared = (cells[i].x - cells[j].x).powi(2) + (cells[i].y - cells[j].y).powi(2);
                let reach = cells[i].values[6] + cells[j].values[6];
                if squared > 0.0 && squared < reach * reach {
                    expected.push((i as u32, j as u32));
                }
            }
        }
        assert_eq!(actual, expected);
    }

    #[test]
    fn ciliary_animation_keeps_reverse_phase_negative() {
        let period = 0.558_505_36;
        assert_eq!(wrap_ciliary_phase(-0.1), -0.1);
        assert_eq!(wrap_ciliary_phase(0.1), 0.1);
        assert!((wrap_ciliary_phase(-period - 0.02) + 0.02).abs() < 1e-6);
        assert!((wrap_ciliary_phase(period + 0.02) - 0.02).abs() < 1e-6);
    }

    #[test]
    fn overlapping_cells_record_reciprocal_contact_chords() {
        let cell = |x: f32, radius: f32| RenderCell {
            runtime_id: 0,
            x,
            y: 0.0,
            radius,
            angle: 0.0,
            phase: 0.0,
            reserve: 0.0,
            signals: [0.0; 2],
            activity: [0.0; 2],
            animation_phase: 0.0,
            auxiliary: [0.0; 4],
            mass: 0.0,
            age: 0.0,
            nitrogen: 0.0,
            toxins: 0.0,
            injury: 0.0,
            lift: 0.0,
            signal_levels: [0.0; 4],
            mirrored: false,
            mutations: 0,
            telomeres: -1,
            color: [0.0; 3],
            stored_color: [0.0; 3],
            cell_type: 0,
            origin: 0,
            mode: 0,
            contact_chords: Vec::new(),
        };
        let mut cells = vec![cell(0.0, 0.02), cell(0.03, 0.02), cell(0.10, 0.02)];
        add_contact_chords(&mut cells, |_| true);
        assert_eq!(cells[0].contact_chords.len(), 1);
        assert_eq!(cells[1].contact_chords.len(), 1);
        assert!(cells[2].contact_chords.is_empty());
        let first = cells[0].contact_chords[0];
        let second = cells[1].contact_chords[0];
        assert!((first[0] - 1.0).abs() < 1e-5);
        assert!((second[0] + 1.0).abs() < 1e-5);
        assert!((first[2] - 0.015).abs() < 1e-5);
        assert!((second[2] - 0.015).abs() < 1e-5);

        let mut dense: Vec<RenderCell> = (0..1536)
            .map(|index| {
                let mut render = cell(
                    ((index * 37) % 53) as f32 * 0.01,
                    0.015 + (index % 4) as f32 * 0.005,
                );
                render.y = ((index * 43) % 47) as f32 * 0.01;
                render
            })
            .collect();
        let mut hashed = dense.clone();
        add_contact_chords(&mut dense, |index| index % 17 != 0);
        add_contact_chords_hash(&mut hashed, |index| index % 17 != 0);
        for (index, (actual, expected)) in dense.iter().zip(&hashed).enumerate() {
            assert_eq!(
                actual.contact_chords, expected.contact_chords,
                "cell {index}"
            );
        }

        let mut sparse = vec![cell(0.0, 0.02), cell(0.03, 0.02), cell(10_000.0, 0.02)];
        let mut sparse_hash = sparse.clone();
        add_contact_chords(&mut sparse, |_| true);
        add_contact_chords_hash(&mut sparse_hash, |_| true);
        for (actual, expected) in sparse.iter().zip(&sparse_hash) {
            assert_eq!(actual.contact_chords, expected.contact_chords);
        }
    }

    #[test]
    fn inspector_snapshot_tracks_live_cell_state() {
        let mut world = legacy::import(bundled::PLATES[0].1).unwrap();
        world.cells.truncate(1);
        world.cells[0].links.clear();
        world.cells[0].c = 0;
        let cell = &mut world.cells[0];
        cell.values[8] = 12.5;
        cell.tuvp[3] = 0.73;
        cell.tail[0] = 0.4;
        cell.tail[1] = 0.2;
        cell.q = 0.6;
        cell.u = [0.1, -0.2, 0.3, -0.4];
        cell.y_flag = true;
        cell.g = 4;
        cell.t = 23;
        cell.e = 4;
        let rendered = &snapshot(&world, false, 1.0).cells[0];
        assert_eq!(rendered.age, 12.5);
        assert_eq!(rendered.nitrogen, 0.73_f32);
        assert_eq!(rendered.toxins, 0.4);
        assert_eq!(rendered.injury, 0.2);
        assert_eq!(rendered.lift, 0.6_f32);
        assert_eq!(rendered.signal_levels, [0.1, -0.2, 0.3, -0.4]);
        assert!(rendered.mirrored);
        assert_eq!(rendered.mutations, 4);
        assert_eq!(rendered.telomeres, 23);
        assert_eq!(rendered.origin, 4);
        world.cells[0].values[8] = 13.0;
        assert_eq!(snapshot(&world, false, 1.0).cells[0].age, 13.0);
    }

    #[test]
    fn cells_marked_for_removal_do_not_create_contact_chords() {
        let mut world = legacy::import(bundled::PLATES[0].1).unwrap();
        let original = world.cells[0].clone();
        world.cells = vec![original.clone(), original];
        world.cells[0].x = 0.0;
        world.cells[1].x = 0.02;
        world.cells[0].y = 0.0;
        world.cells[1].y = 0.0;
        world.cells[0].values[6] = 0.02;
        world.cells[1].values[6] = 0.02;
        world.cells[0].x_flag = false;
        world.cells[1].x_flag = false;
        let touching = snapshot(&world, false, 1.0);
        assert!(
            touching
                .cells
                .iter()
                .all(|cell| cell.contact_chords.len() == 1)
        );
        world.cells[1].x_flag = true;
        let render = snapshot(&world, false, 1.0);
        assert!(
            render
                .cells
                .iter()
                .all(|cell| cell.contact_chords.is_empty())
        );
    }

    #[test]
    fn simulation_worker_stops_when_its_command_channel_closes() {
        let world = legacy::import(bundled::PLATES[0].1).unwrap();
        let Engine {
            commands, events, ..
        } = start(world);
        drop(commands);
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            assert!(!remaining.is_zero(), "simulation worker did not stop");
            match events.recv_timeout(remaining) {
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Err(mpsc::RecvTimeoutError::Timeout) => panic!("simulation worker did not stop"),
                Ok(_) => {}
            }
        }
    }

    #[test]
    fn worker_saves_plate_edits_in_command_order() {
        let mut world = legacy::import(bundled::PLATES[0].1).unwrap();
        world.cells.truncate(5);
        world.food.truncate(6);
        let mut settings = world.environment.clone();
        settings.d = 1.6;
        settings.c = 0.02;
        settings.w = true;
        settings.enabled_types.fill(false);
        settings.enabled_types[2] = true;
        let engine = start(world);
        let path = std::env::temp_dir().join(format!(
            "celllab2-worker-edits-{}-{}.celllab2",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        for command in [
            Command::Radius(1.75),
            Command::MaxCells(432),
            Command::MaxFood(678),
            Command::FoodCount(9),
            Command::EnvironmentSettings(settings),
            Command::Save(path.to_string_lossy().into_owned()),
        ] {
            engine.commands.send(command).unwrap();
        }
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            assert!(
                !remaining.is_zero(),
                "worker did not finish saving plate edits"
            );
            match engine.events.recv_timeout(remaining).unwrap() {
                Event::Message(message) if message.starts_with("Saved ") => break,
                Event::Snapshot(_) => engine.acknowledge_snapshot(),
                _ => {}
            }
        }
        let saved = native::load(&path).unwrap();
        std::fs::remove_file(path).unwrap();
        assert_eq!(saved.environment.radius, 1.75);
        assert_eq!(saved.environment.max_cells, 432);
        assert_eq!(saved.environment.max_food, 678);
        assert_eq!(saved.environment.d, 1.6);
        assert_eq!(saved.environment.c, 0.02);
        assert!(saved.environment.w);
        assert!(saved.environment.enabled_types[2]);
        assert_eq!(
            saved
                .environment
                .enabled_types
                .iter()
                .filter(|x| **x)
                .count(),
            1
        );
        assert_eq!(saved.cells.len(), 5);
        assert_eq!(saved.food.len(), 9);
    }

    #[test]
    fn worker_reports_save_after_event_queue_fills() {
        let mut world = legacy::import(bundled::PLATES[0].1).unwrap();
        world.cells.truncate(1);
        world.food.clear();
        let id = world.cells[0].runtime_id;
        let engine = start(world);
        match engine.events.recv_timeout(Duration::from_secs(2)).unwrap() {
            Event::Snapshot(_) => engine.acknowledge_snapshot(),
            _ => panic!("expected initial snapshot"),
        }
        let path = std::env::temp_dir().join(format!(
            "celllab2-save-event-{}-{}.celllab2",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        for _ in 0..4 {
            engine.commands.send(Command::InspectCell(id)).unwrap();
        }
        engine
            .commands
            .send(Command::Save(path.to_string_lossy().into_owned()))
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        while !path.exists() {
            assert!(Instant::now() < deadline, "worker did not write the save");
            thread::sleep(Duration::from_millis(5));
        }
        thread::sleep(Duration::from_millis(50));
        let mut inspected = 0;
        let mut saved = false;
        while Instant::now() < deadline && !saved {
            match engine
                .events
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            {
                Ok(Event::Inspected(..)) => inspected += 1,
                Ok(Event::Message(message)) if message.starts_with("Saved ") => saved = true,
                Ok(Event::Snapshot(_)) => engine.acknowledge_snapshot(),
                Ok(_) => {}
                Err(_) => break,
            }
        }
        assert_eq!(inspected, 4);
        assert!(saved, "worker dropped the save confirmation");
        assert_eq!(native::load(&path).unwrap().cells.len(), 1);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn worker_targets_cells_by_identity_after_compaction() {
        let mut world = legacy::import(bundled::PLATES[0].1).unwrap();
        world.cells.truncate(2);
        world.cells[0].x = -0.3;
        world.cells[1].x = 0.3;
        let removed_id = world.cells[0].runtime_id;
        let surviving_id = world.cells[1].runtime_id;
        assert_ne!(removed_id, surviving_id);
        let engine = start(world);
        let path = std::env::temp_dir().join(format!(
            "celllab2-cell-identity-{}-{}.celllab2",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        for command in [
            Command::RemoveCell(removed_id),
            Command::BoostCell(surviving_id),
            Command::RemoveCell(removed_id),
            Command::InspectCell(surviving_id),
            Command::Save(path.to_string_lossy().into_owned()),
        ] {
            engine.commands.send(command).unwrap();
        }
        let mut removed = 0;
        let mut rejected = 0;
        let mut inspected = false;
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            assert!(
                !remaining.is_zero(),
                "worker did not finish identity commands"
            );
            match engine.events.recv_timeout(remaining).unwrap() {
                Event::CellRemoved => removed += 1,
                Event::CellRemoveRejected => rejected += 1,
                Event::Inspected(id, cell) => {
                    assert_eq!(id, surviving_id);
                    assert_eq!(cell.x, 0.3);
                    assert_eq!(cell.color, [1.0, 0.0, 1.0]);
                    inspected = true;
                }
                Event::Message(message) if message.starts_with("Saved ") => break,
                Event::Snapshot(_) => engine.acknowledge_snapshot(),
                _ => {}
            }
        }
        let saved = native::load(&path).unwrap();
        std::fs::remove_file(path).unwrap();
        assert_eq!(removed, 1);
        assert_eq!(rejected, 1);
        assert!(inspected);
        assert_eq!(saved.cells.len(), 1);
        assert_eq!(saved.cells[0].x, 0.3);
        assert_eq!(saved.cells[0].color, [1.0, 0.0, 1.0]);
    }

    #[test]
    fn worker_assigns_distinct_ids_to_cloned_cells() {
        let mut world = legacy::import(bundled::PLATES[0].1).unwrap();
        let template = world.cells[0].clone();
        world.cells = vec![template.clone(), template];
        let engine = start(world);
        let snapshot = match engine.events.recv_timeout(Duration::from_secs(2)).unwrap() {
            Event::Snapshot(snapshot) => snapshot,
            _ => panic!("expected an initial render snapshot"),
        };
        assert_eq!(snapshot.cells.len(), 2);
        assert_ne!(snapshot.cells[0].runtime_id, snapshot.cells[1].runtime_id);
        assert_ne!(snapshot.cells[0].runtime_id, 0);
        assert_ne!(snapshot.cells[1].runtime_id, 0);
    }

    #[test]
    fn placement_at_cell_cap_replaces_one_organism() {
        let mut world = legacy::import(bundled::PLATES[0].1).unwrap();
        world.cells.truncate(2);
        world.cells[0].x = -0.3;
        world.cells[1].x = 0.3;
        world.environment.max_cells = 2;
        let genes = world.cells[0].genes.clone();
        let engine = start(world);
        let path = std::env::temp_dir().join(format!(
            "celllab2-cap-placement-{}-{}.celllab2",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        engine
            .commands
            .send(Command::AddCell(0.1, 0.0, None, Some(genes), None))
            .unwrap();
        engine
            .commands
            .send(Command::Save(path.to_string_lossy().into_owned()))
            .unwrap();
        let mut added = false;
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            assert!(!remaining.is_zero(), "worker did not save cap placement");
            match engine.events.recv_timeout(remaining).unwrap() {
                Event::CellAdded => added = true,
                Event::CellAddRejected => panic!("placement at cap was rejected"),
                Event::Message(message) if message.starts_with("Saved ") => break,
                Event::Snapshot(_) => engine.acknowledge_snapshot(),
                _ => {}
            }
        }
        let saved = native::load(&path).unwrap();
        std::fs::remove_file(path).unwrap();
        assert!(added);
        assert_eq!(saved.cells.len(), 2);
        assert_eq!(saved.environment.max_cells, 2);
        assert!(saved.cells.iter().any(|cell| cell.x == 0.1 && cell.e == 1));
    }

    #[test]
    fn cap_replacement_retains_links_to_swapped_survivor() {
        let mut world = legacy::import(bundled::PLATES[0].1).unwrap();
        world.cells.truncate(3);
        let moved_id = world.cells[2].runtime_id;
        world.cells[0].links = vec![
            Link {
                target: 1,
                c: 0.0,
                d: 0.0,
                b: false,
                e: 0.0,
                f: 0.0,
                k: 0.0,
                l: 0.0,
            },
            Link {
                target: 2,
                c: 0.0,
                d: 0.0,
                b: false,
                e: 0.0,
                f: 0.0,
                k: 0.0,
                l: 0.0,
            },
        ];
        world.cells[0].c = 2;
        evict_cell_for_placement(&mut world, 1);
        assert_eq!(world.cells.len(), 2);
        assert_eq!(world.cells[1].runtime_id, moved_id);
        assert_eq!(world.cells[0].c, 1);
        assert_eq!(world.cells[0].links[0].target, 1);
        assert!(world.dead.is_empty());
    }

    #[test]
    fn simulation_waits_for_the_ui_to_consume_a_snapshot() {
        let world = legacy::import(bundled::PLATES[0].1).unwrap();
        let engine = start(world);
        let first_time = match engine.events.recv_timeout(Duration::from_secs(2)).unwrap() {
            Event::Snapshot(snapshot) => snapshot.time,
            _ => panic!("expected an initial render snapshot"),
        };
        engine.commands.send(Command::Step).unwrap();
        thread::sleep(Duration::from_millis(120));
        assert!(matches!(engine.events.try_recv(), Err(TryRecvError::Empty)));
        engine.acknowledge_snapshot();
        let updated_time = match engine.events.recv_timeout(Duration::from_secs(2)).unwrap() {
            Event::Snapshot(snapshot) => snapshot.time,
            _ => panic!("expected the updated render snapshot"),
        };
        assert!(updated_time > first_time);
    }

    #[test]
    fn simulation_accepts_high_speed_and_uncapped_incubation() {
        let mut world = legacy::import(bundled::PLATES[0].1).unwrap();
        world.cells.clear();
        world.food.clear();
        world.environment.max_food = 0;
        world.environment.w = false;
        let engine = start(world);
        assert!(matches!(
            engine.events.recv_timeout(Duration::from_secs(2)).unwrap(),
            Event::Snapshot(_)
        ));
        engine.acknowledge_snapshot();
        engine.commands.send(Command::Speed(1_000.0)).unwrap();
        engine.commands.send(Command::Play(true)).unwrap();
        let fast = match engine.events.recv_timeout(Duration::from_secs(2)).unwrap() {
            Event::Snapshot(snapshot) => snapshot,
            _ => panic!("expected a high-speed snapshot"),
        };
        assert_eq!(fast.speed, 1_000.0);
        assert!(fast.time > 0.0);
        engine.acknowledge_snapshot();

        engine.commands.send(Command::Speed(f64::INFINITY)).unwrap();
        let incubating = match engine.events.recv_timeout(Duration::from_secs(2)).unwrap() {
            Event::Snapshot(snapshot) => snapshot,
            _ => panic!("expected an incubating snapshot"),
        };
        assert_eq!(incubating.speed, f64::INFINITY);
        assert!(incubating.time > fast.time);
        engine.acknowledge_snapshot();
        engine.commands.send(Command::Play(false)).unwrap();
        engine.commands.send(Command::Speed(1.0)).unwrap();
        let paused = match engine.events.recv_timeout(Duration::from_secs(2)).unwrap() {
            Event::Snapshot(snapshot) => snapshot,
            _ => panic!("expected a paused snapshot"),
        };
        assert!(!paused.running);
        assert_eq!(paused.speed, 1.0);
    }

    #[test]
    fn incubation_catches_a_goal_that_expires_between_snapshots() {
        let mut world = legacy::import(bundled::PLATES[0].1).unwrap();
        world.cells.clear();
        world.food.clear();
        world.environment.max_food = 0;
        world.environment.w = false;
        let engine = start(world);
        // Leave the first render snapshot pending, as it would be during a slow UI frame.
        engine
            .commands
            .send(Command::WatchChallenge(Some((
                1,
                vec![Condition {
                    tag: "max-time".into(),
                    target: 0.04,
                }],
                0.04,
            ))))
            .unwrap();
        engine.commands.send(Command::Speed(f64::INFINITY)).unwrap();
        engine.commands.send(Command::Play(true)).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut won = false;
        while Instant::now() < deadline {
            match engine
                .events
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .unwrap()
            {
                Event::ChallengeWon(1) => {
                    won = true;
                    break;
                }
                Event::Snapshot(_) => engine.acknowledge_snapshot(),
                _ => {}
            }
        }
        assert!(won, "incubation skipped a transient challenge goal");
        let final_snapshot = loop {
            match engine.events.recv_timeout(Duration::from_secs(2)).unwrap() {
                Event::Snapshot(snapshot) => break snapshot,
                _ => {}
            }
        };
        assert!(!final_snapshot.running);
        assert!(final_snapshot.time <= 0.04);
    }

    #[test]
    fn light_gradient_matches_the_plate_light_field() {
        let radius = 2.5;
        let angle = 0.7;
        let intensity = 0.8;
        let diffusion = 0.35;
        let epsilon = 1.0e-6;
        for (x, y) in [(0.31, -0.24), (-0.41, 0.12), (0.2, 0.4)] {
            let (dx, dy) = light_gradient(x, y, radius, angle, intensity, diffusion);
            let numerical_x = (light_at(x + epsilon, y, radius, angle, intensity, diffusion)
                - light_at(x - epsilon, y, radius, angle, intensity, diffusion))
                / (2.0 * epsilon);
            let numerical_y = (light_at(x, y + epsilon, radius, angle, intensity, diffusion)
                - light_at(x, y - epsilon, radius, angle, intensity, diffusion))
                / (2.0 * epsilon);
            assert!((dx - numerical_x).abs() < 1.0e-7, "x={x}, y={y}");
            assert!((dy - numerical_y).abs() < 1.0e-7, "x={x}, y={y}");
        }
        assert_eq!(light_gradient(-2.0, 0.0, 2.5, 0.0, 0.8, 0.1), (0.0, 0.0));
    }

    #[test]
    fn synthesizer_uses_own_genome_and_apk_starting_values() {
        let world = legacy::import(bundled::PLATES[0].1).unwrap();
        let mut genes = world.cells[0].genes.clone();
        for gene in &mut genes {
            gene.flags[4] = false;
        }
        genes[2].flags[4] = true;
        genes[2].cell_type = 4;
        genes[2].color = [0.2, 0.4, 0.6];
        let cell = synthesize_cell(0.3, -0.2, genes).unwrap();
        assert_eq!(cell.mode, 2);
        assert_eq!(cell.cell_type(), 4);
        assert_eq!(cell.values[7], 1.728);
        assert_eq!(cell.values[6], 0.0297);
        assert_eq!(cell.values[8], 0.1);
        assert_eq!(cell.tuvp[3], 1.0);
        assert_eq!(cell.e, 1);
        assert_eq!(cell.color, [0.2, 0.4, 0.6]);
        assert!(cell.links.is_empty());
        assert_eq!(cell.c, 0);
    }

    #[test]
    fn first_challenge_photocyte_can_grow_and_divide_in_light() {
        let level = crate::levels::bundled()
            .into_iter()
            .find(|level| level.id == "1")
            .unwrap();
        let (_, bytes) = bundled::PLATES
            .iter()
            .find(|(name, _)| *name == level.plate)
            .unwrap();
        let mut world = legacy::import(bytes).unwrap();
        assert!(world.cells.is_empty());
        let x = world.environment.radius * 0.8 * world.light_angle.cos();
        let y = world.environment.radius * 0.8 * world.light_angle.sin();
        assert!(
            light_at(
                x,
                y,
                world.environment.radius,
                world.light_angle,
                world.environment.d,
                world.environment.f,
            ) > 0.0
        );
        let cell = synthesize_cell(x, y, level.starting_genome()).unwrap();
        assert_eq!(cell.cell_type(), 2);
        world.cells.push(cell);
        for _ in 0..100 {
            step(&mut world, 0.02);
            if world.cells.len() > 1 {
                break;
            }
        }
        assert!(world.cells.len() > 1, "starter organism did not divide");
        assert!(world.cells.iter().all(|cell| cell.e == 1));
    }

    #[test]
    #[ignore = "full challenge simulation"]
    fn first_challenge_can_reach_its_hundred_organism_goal() {
        let level = crate::levels::bundled()
            .into_iter()
            .find(|level| level.id == "1")
            .unwrap();
        let (_, bytes) = bundled::PLATES
            .iter()
            .find(|(name, _)| *name == level.plate)
            .unwrap();
        let mut world = legacy::import(bytes).unwrap();
        let mut genes = level.starting_genome();
        // The player can raise this threshold in the genome editor. It lets
        // daughters build enough mass before dividing in this dim plate.
        genes[0].floats[0] = 0.34;
        for index in 0..level.place_limit {
            let angle = world.light_angle - 0.45 + 0.1 * index as f64;
            let x = world.environment.radius * 0.93 * angle.cos();
            let y = world.environment.radius * 0.93 * angle.sin();
            world
                .cells
                .push(synthesize_cell(x, y, genes.clone()).unwrap());
        }
        for _ in 0..5_000 {
            step(&mut world, 0.02);
            let user_count = world.cells.iter().filter(|cell| cell.e == 1).count();
            if user_count >= 100 {
                let achieved = snapshot(&world, false, 1.0);
                assert!(
                    level
                        .conditions
                        .iter()
                        .all(|condition| crate::levels::condition_met(condition, &achieved))
                );
                eprintln!("level 1 goal reached after {:.2} hours", world.time);
                return;
            }
        }
        panic!(
            "level 1 goal not reached: {} user organisms after {:.1} hours",
            world.cells.iter().filter(|cell| cell.e == 1).count(),
            world.time
        );
    }

    #[test]
    #[ignore = "full challenge simulation"]
    fn second_challenge_can_reach_its_hundred_fifty_organism_goal() {
        let level = crate::levels::bundled()
            .into_iter()
            .find(|level| level.id == "2")
            .unwrap();
        let (_, bytes) = bundled::PLATES
            .iter()
            .find(|(name, _)| *name == level.plate)
            .unwrap();
        let mut world = legacy::import(bytes).unwrap();
        let mut genes = level.starting_genome();
        assert!(level.allowed_types[2]);
        assert!(!level.allowed_types[0]);
        assert_eq!(world.environment.a, 0.0);
        // Earlier division and wider placement fill the lit rim without food.
        genes[0].floats[0] = 0.20;
        for index in 0..level.place_limit {
            let angle = world.light_angle - 0.9 + 0.2 * index as f64;
            let x = world.environment.radius * 0.93 * angle.cos();
            let y = world.environment.radius * 0.93 * angle.sin();
            assert!(level.allows_genes(&genes));
            world
                .cells
                .push(synthesize_cell(x, y, genes.clone()).unwrap());
        }
        let mut peak = 0;
        for _ in 0..5_000 {
            step(&mut world, 0.02);
            let user_count = world.cells.iter().filter(|cell| cell.e == 1).count();
            peak = peak.max(user_count);
            if level.conditions.iter().all(|condition| {
                crate::levels::condition_met(condition, &snapshot(&world, false, 1.0))
            }) {
                eprintln!("level 2 goal reached after {:.2} hours", world.time);
                return;
            }
        }
        panic!(
            "level 2 goal not reached; peak {peak}, final {} after {:.1} hours",
            world.cells.len(),
            world.time
        );
    }

    #[test]
    #[ignore = "full challenge simulation"]
    fn fourth_challenge_can_reach_its_hundred_twenty_organism_goal() {
        let level = crate::levels::bundled()
            .into_iter()
            .find(|level| level.id == "4")
            .unwrap();
        let (_, bytes) = bundled::PLATES
            .iter()
            .find(|(name, _)| *name == level.plate)
            .unwrap();
        let mut world = legacy::import(bytes).unwrap();
        let mut genes = level.starting_genome();
        genes[0].floats[0] = 0.20;
        // The APK hint recommends making adhesin in this low-viscosity plate.
        genes[0].flags[0] = true;
        assert!(level.allows_genes(&genes));
        for index in 0..level.place_limit {
            let angle = world.light_angle - 0.8 + 0.4 * index as f64;
            let x = world.environment.radius * 0.93 * angle.cos();
            let y = world.environment.radius * 0.93 * angle.sin();
            world
                .cells
                .push(synthesize_cell(x, y, genes.clone()).unwrap());
        }
        let mut peak = 0;
        for _ in 0..5_000 {
            step(&mut world, 0.02);
            let user_count = world.cells.iter().filter(|cell| cell.e == 1).count();
            peak = peak.max(user_count);
            if level.conditions.iter().all(|condition| {
                crate::levels::condition_met(condition, &snapshot(&world, false, 1.0))
            }) {
                eprintln!("level 4 goal reached after {:.2} hours", world.time);
                return;
            }
        }
        panic!(
            "level 4 goal not reached; peak {peak}, final {} after {:.1} hours",
            world.cells.len(),
            world.time
        );
    }

    #[test]
    #[ignore = "full challenge simulation"]
    fn macrophages_challenge_can_reach_its_three_hundred_organism_goal() {
        let level = crate::levels::bundled()
            .into_iter()
            .find(|level| level.id == "3")
            .unwrap();
        let (_, bytes) = bundled::PLATES
            .iter()
            .find(|(name, _)| *name == level.plate)
            .unwrap();
        let mut world = legacy::import(bytes).unwrap();
        let genes = level.starting_genome();
        assert_eq!(genes[0].cell_type, 0);
        assert!(level.allows_genes(&genes));
        for index in 0..level.place_limit {
            let radius = world.environment.radius
                * 0.8
                * ((index as f64 + 0.5) / level.place_limit as f64).sqrt();
            let angle = index as f64 * 2.399_963_229_728_653;
            world.cells.push(
                synthesize_cell(radius * angle.cos(), radius * angle.sin(), genes.clone()).unwrap(),
            );
        }
        let mut peak = 0;
        for _ in 0..5_000 {
            step(&mut world, 0.02);
            let user_count = world.cells.iter().filter(|cell| cell.e == 1).count();
            peak = peak.max(user_count);
            if level.conditions.iter().all(|condition| {
                crate::levels::condition_met(condition, &snapshot(&world, false, 1.0))
            }) {
                eprintln!("macrophages goal reached after {:.2} hours", world.time);
                return;
            }
        }
        panic!(
            "macrophages goal not reached; peak {peak}, final {} after {:.1} hours",
            world.cells.len(),
            world.time
        );
    }

    #[test]
    #[ignore = "full challenge simulation"]
    fn macrophages_two_can_reach_its_six_hundred_fifty_organism_goal() {
        let level = crate::levels::bundled()
            .into_iter()
            .find(|level| level.id == "3.2")
            .unwrap();
        let (_, bytes) = bundled::PLATES
            .iter()
            .find(|(name, _)| *name == level.plate)
            .unwrap();
        let mut world = legacy::import(bytes).unwrap();
        let mut genes = level.starting_genome();
        assert_eq!(level.place_limit, 1);
        assert!(level.allows_genes(&genes));
        genes[0].floats[2] = 1.05;
        genes[0].floats[1] = 0.42;
        world.cells.push(synthesize_cell(0.0, 0.0, genes).unwrap());
        let mut peak = 0;
        for _ in 0..5_000 {
            step(&mut world, 0.02);
            let user_count = world.cells.iter().filter(|cell| cell.e == 1).count();
            peak = peak.max(user_count);
            if level.conditions.iter().all(|condition| {
                crate::levels::condition_met(condition, &snapshot(&world, false, 1.0))
            }) {
                eprintln!("macrophages II goal reached after {:.2} hours", world.time);
                return;
            }
        }
        panic!(
            "macrophages II goal not reached; peak {peak}, final {} after {:.1} hours",
            world.cells.len(),
            world.time
        );
    }

    #[test]
    #[ignore = "full challenge simulation"]
    fn macrophages_three_can_reach_four_hundred_twenty_with_three_modes() {
        let level = crate::levels::bundled()
            .into_iter()
            .find(|level| level.id == "5")
            .unwrap();
        let index = bundled::PLATES
            .iter()
            .position(|(name, _)| *name == level.plate)
            .unwrap();
        let mut world = load_bundled(index, true).unwrap();
        assert_eq!(level.place_limit, 10);
        let mut genes = level.starting_genome();
        genes[0].cell_type = 0;
        genes[1].cell_type = 1;
        genes[2].cell_type = 0;
        genes[0].i = 1;
        genes[0].j = 2;
        genes[0].flags[0] = true;
        genes[2].i = 2;
        genes[2].j = 0;
        genes[1].inputs[0].b = 0.4;
        assert!(level.allows_genes(&genes));
        for index in 0..level.place_limit {
            let angle = index as f64 * std::f64::consts::TAU / level.place_limit as f64;
            let radius = world.environment.radius * (0.35 + 0.45 * (index % 2) as f64);
            world.cells.push(
                synthesize_cell(radius * angle.cos(), radius * angle.sin(), genes.clone()).unwrap(),
            );
        }
        let mut peak = 0;
        for _ in 0..10_000 {
            step(&mut world, 0.02);
            let user_count = world.cells.iter().filter(|cell| cell.e == 1).count();
            peak = peak.max(user_count);
            if user_count >= 420 {
                let achieved = snapshot(&world, false, 1.0);
                let swimmers = world
                    .cells
                    .iter()
                    .filter(|cell| cell.cell_type() == 1)
                    .count();
                assert!(
                    swimmers > 0,
                    "the three-mode lineage lost its Flagellocytes"
                );
                assert!(
                    level
                        .conditions
                        .iter()
                        .all(|condition| { crate::levels::condition_met(condition, &achieved) })
                );
                eprintln!(
                    "macrophages III reached 420 cells with {swimmers} swimmers after {:.1}h",
                    world.time
                );
                return;
            }
        }
        panic!(
            "macrophages III goal not reached: peak {peak}, final {} after {:.1}h",
            world.cells.len(),
            world.time
        );
    }

    #[test]
    #[ignore = "unverified: inherited Macrophages III genome peaks below 300"]
    fn macrophages_four_can_reach_three_hundred_with_swimmers() {
        let level = crate::levels::bundled()
            .into_iter()
            .find(|level| level.id == "5.5")
            .unwrap();
        let index = bundled::PLATES
            .iter()
            .position(|(name, _)| *name == level.plate)
            .unwrap();
        let mut world = load_bundled(index, true).unwrap();
        assert_eq!(level.place_limit, 30);
        let mut genes = level.starting_genome();
        genes[0].cell_type = 0;
        genes[1].cell_type = 1;
        genes[2].cell_type = 0;
        genes[0].i = 1;
        genes[0].j = 2;
        genes[0].flags[0] = true;
        genes[2].i = 2;
        genes[2].j = 0;
        genes[1].inputs[0].b = 0.4;
        genes[0].floats[0] = 0.20;
        genes[2].floats[0] = 0.20;
        assert!(level.allows_genes(&genes));
        for index in 0..level.place_limit {
            let angle = index as f64 * 2.399_963_229_728_653;
            let radius = world.environment.radius * (0.25 + 0.22 * (index % 3) as f64);
            world.cells.push(
                synthesize_cell(radius * angle.cos(), radius * angle.sin(), genes.clone()).unwrap(),
            );
        }
        let mut peak = 0;
        for _ in 0..10_000 {
            step(&mut world, 0.02);
            let user_count = world.cells.iter().filter(|cell| cell.e == 1).count();
            peak = peak.max(user_count);
            if user_count >= 300 {
                let achieved = snapshot(&world, false, 1.0);
                assert!(
                    level
                        .conditions
                        .iter()
                        .all(|condition| { crate::levels::condition_met(condition, &achieved) })
                );
                eprintln!("macrophages IV reached 300 cells after {:.1}h", world.time);
                return;
            }
        }
        panic!(
            "macrophages IV goal not reached: peak {peak}, final {} after {:.1}h",
            world.cells.len(),
            world.time
        );
    }

    #[test]
    #[ignore = "full challenge simulation"]
    fn seed_challenge_lipocytes_hold_sixteen_cells_without_food() {
        let level = crate::levels::bundled()
            .into_iter()
            .find(|level| level.id == "6.5")
            .unwrap();
        let index = bundled::PLATES
            .iter()
            .position(|(name, _)| *name == level.plate)
            .unwrap();
        let mut world = load_bundled(index, true).unwrap();
        assert_eq!(level.place_limit, 1);
        assert!(world.cells.is_empty());
        assert!(world.food.is_empty());
        assert_eq!(world.environment.a, 0.0);
        let mut genes = level.starting_genome();
        genes[0].cell_type = 4;
        genes[0].floats[0] = 0.085;
        assert!(level.allows_genes(&genes));
        world.cells.push(synthesize_cell(0.0, 0.0, genes).unwrap());
        let mut held_since = None;
        let mut peak = 0;
        for _ in 0..10_000 {
            step(&mut world, 0.02);
            let user_count = world.cells.iter().filter(|cell| cell.e == 1).count();
            peak = peak.max(user_count);
            if user_count >= 16 {
                let since = *held_since.get_or_insert(world.time);
                if world.time - since >= level.hold_time {
                    let achieved = snapshot(&world, false, 1.0);
                    assert!(
                        level.conditions.iter().all(|condition| {
                            crate::levels::condition_met(condition, &achieved)
                        })
                    );
                    assert!(world.cells.iter().all(|cell| cell.cell_type() == 4));
                    eprintln!(
                        "Seed held 16 Lipocytes without food until {:.1}h",
                        world.time
                    );
                    return;
                }
            } else {
                held_since = None;
            }
        }
        panic!(
            "Seed goal not reached: peak {peak}, final {} after {:.1}h",
            world.cells.len(),
            world.time
        );
    }

    #[test]
    #[ignore = "full live challenge simulation"]
    fn seed_challenge_wins_through_worker_incubation() {
        let level = crate::levels::bundled()
            .into_iter()
            .find(|level| level.id == "6.5")
            .unwrap();
        let index = bundled::PLATES
            .iter()
            .position(|(name, _)| *name == level.plate)
            .unwrap();
        let world = load_bundled(index, true).unwrap();
        let mut genes = level.starting_genome();
        genes[0].cell_type = 4;
        genes[0].floats[0] = 0.085;
        assert!(level.allows_genes(&genes));
        let engine = start(world);
        assert!(matches!(
            engine.events.recv_timeout(Duration::from_secs(2)).unwrap(),
            Event::Snapshot(_)
        ));
        engine.acknowledge_snapshot();
        let run_id = 0x5EED_u64;
        engine
            .commands
            .send(Command::WatchChallenge(Some((
                run_id,
                level.conditions.clone(),
                level.hold_time,
            ))))
            .unwrap();
        engine
            .commands
            .send(Command::AddCell(
                0.0,
                0.0,
                None,
                Some(genes),
                Some(level.gene_limits()),
            ))
            .unwrap();
        engine.commands.send(Command::Speed(f64::INFINITY)).unwrap();
        engine.commands.send(Command::Play(true)).unwrap();
        let deadline = Instant::now() + Duration::from_secs(30);
        let mut placed = false;
        let mut won = false;
        let mut stopped = false;
        while Instant::now() < deadline {
            match engine.events.recv_timeout(Duration::from_millis(500)) {
                Ok(Event::Snapshot(snapshot)) => {
                    engine.acknowledge_snapshot();
                    if won {
                        stopped = !snapshot.running;
                        break;
                    }
                }
                Ok(Event::CellAdded) => placed = true,
                Ok(Event::ChallengeWon(id)) => {
                    assert_eq!(id, run_id);
                    won = true;
                }
                Ok(Event::Message(message)) => panic!("worker reported: {message}"),
                Ok(Event::CellAddRejected) => panic!("legal Seed genome was rejected"),
                Ok(_) | Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(error) => panic!("worker stopped before Seed win: {error}"),
            }
        }
        assert!(placed, "worker did not place the Seed cell");
        assert!(won, "worker did not emit the Seed challenge win");
        assert!(stopped, "worker did not pause after the Seed win");
    }

    #[test]
    #[ignore = "unverified: tested Scavenger genome goes extinct before 100h hold"]
    fn scavenger_challenge_feeds_on_native_photocyte_remains() {
        let level = crate::levels::bundled()
            .into_iter()
            .find(|level| level.id == "6.2")
            .unwrap();
        let index = bundled::PLATES
            .iter()
            .position(|(name, _)| *name == level.plate)
            .unwrap();
        let mut world = load_bundled(index, true).unwrap();
        assert_eq!(level.place_limit, 5);
        assert_eq!(world.environment.a, 0.0);
        assert!(world.cells.len() > 500);
        assert!(world.food.len() > 400);
        assert!(world.food.iter().all(|food| food.state <= 0.0));
        let mut genes = level.starting_genome();
        genes[0].cell_type = 0;
        genes[1].cell_type = 1;
        genes[2].cell_type = 0;
        genes[0].i = 1;
        genes[0].j = 2;
        genes[0].flags[0] = true;
        genes[2].i = 2;
        genes[2].j = 0;
        genes[1].inputs[0].b = 0.4;
        assert!(level.allows_genes(&genes));
        let mut candidates: Vec<_> = world
            .food
            .iter()
            .map(|food| {
                let density = world
                    .food
                    .iter()
                    .filter(|other| (food.x - other.x).hypot(food.y - other.y) < 0.08)
                    .count();
                (density, food.x as f64, food.y as f64)
            })
            .collect();
        candidates.sort_unstable_by(|a, b| b.0.cmp(&a.0));
        let mut placements: Vec<(f64, f64)> = Vec::new();
        for (_, x, y) in candidates {
            if placements
                .iter()
                .all(|(px, py)| (x - px).hypot(y - py) > 0.18)
            {
                placements.push((x, y));
                world
                    .cells
                    .push(synthesize_cell(x, y, genes.clone()).unwrap());
                if placements.len() == level.place_limit {
                    break;
                }
            }
        }
        assert_eq!(placements.len(), level.place_limit);
        let mut held_since = None;
        let mut peak = 0;
        let mut longest_hold: f64 = 0.0;
        for step_index in 0..15_000 {
            step(&mut world, 0.02);
            let user_count = world.cells.iter().filter(|cell| cell.e == 1).count();
            peak = peak.max(user_count);
            if step_index % 1000 == 999 {
                eprintln!(
                    "scavenger {:.0}h: {user_count} user cells, {} food",
                    world.time,
                    world.food.len()
                );
            }
            if user_count == 0 {
                panic!(
                    "Scavenger lineage extinct after {:.1}h; peak {peak}",
                    world.time
                );
            }
            if user_count >= 25 {
                let since = *held_since.get_or_insert(world.time);
                longest_hold = longest_hold.max(world.time - since);
                if world.time - since >= level.hold_time {
                    let achieved = snapshot(&world, false, 1.0);
                    assert!(
                        level.conditions.iter().all(|condition| {
                            crate::levels::condition_met(condition, &achieved)
                        })
                    );
                    eprintln!("Scavenger held 25 cells through {:.1}h", world.time);
                    return;
                }
            } else {
                held_since = None;
            }
        }
        panic!(
            "Scavenger goal not reached: peak {peak}, longest hold {longest_hold:.1}h, final {} after {:.1}h",
            world.cells.iter().filter(|cell| cell.e == 1).count(),
            world.time
        );
    }

    #[test]
    fn challenge_loading_resets_seeded_cell_origin_and_clock() {
        let index = bundled::PLATES
            .iter()
            .position(|(name, _)| *name == "colch")
            .unwrap();
        let plain = load_bundled(index, false).unwrap();
        assert_eq!(plain.cells.len(), 1);
        assert_eq!(plain.cells[0].e, 1);
        assert!(plain.time > 100.0);
        let challenge = load_bundled(index, true).unwrap();
        assert_eq!(challenge.cells.len(), plain.cells.len());
        assert_eq!(challenge.food.len(), plain.food.len());
        assert_eq!(challenge.cells[0].e, 0);
        assert_eq!(challenge.time, 0.0);
        assert_eq!(
            challenge.cells[0].genes[0].color,
            plain.cells[0].genes[0].color
        );
    }

    #[test]
    fn colors_challenge_reaches_exact_red_and_yellow_goal_with_boost() {
        let level = crate::levels::bundled()
            .into_iter()
            .find(|level| level.id == "3.3")
            .unwrap();
        let index = bundled::PLATES
            .iter()
            .position(|(name, _)| *name == level.plate)
            .unwrap();
        let mut world = load_bundled(index, true).unwrap();
        assert_eq!(level.place_limit, 0);
        assert!(level.allows(crate::levels::Tool::Boost));
        assert!(!level.allows(crate::levels::Tool::Cell));
        // Each mode divides into the next two modes in this APK genome.
        for mode in [0, 1, 3, 4, 0, 5, 0, 5, 0] {
            let before = world.cells.len();
            let cell = world
                .cells
                .iter_mut()
                .find(|cell| cell.mode == mode)
                .unwrap();
            boost_cell(cell);
            for _ in 0..100 {
                step(&mut world, 0.02);
                if world.cells.len() > before {
                    break;
                }
            }
            assert_eq!(world.cells.len(), before + 1, "mode {mode} did not divide");
        }
        let achieved = snapshot(&world, false, 1.0);
        assert_eq!(world.cells.len(), 10);
        assert!(
            level
                .conditions
                .iter()
                .all(|condition| crate::levels::condition_met(condition, &achieved))
        );
    }

    #[test]
    #[ignore = "full challenge simulation"]
    fn breeding_challenge_can_reach_hundred_red_native_cells() {
        let level = crate::levels::bundled()
            .into_iter()
            .find(|level| level.id == "3.4")
            .unwrap();
        let index = bundled::PLATES
            .iter()
            .position(|(name, _)| *name == level.plate)
            .unwrap();
        let mut world = load_bundled(index, true).unwrap();
        let red_count = |world: &World| {
            world
                .cells
                .iter()
                .filter(|cell| {
                    cell.e == 0
                        && cell.color[0] > 0.65
                        && cell.color[1] < 0.35
                        && cell.color[2] < 0.35
                })
                .count()
        };
        assert_eq!(level.place_limit, 0);
        for tool in [
            crate::levels::Tool::Remove,
            crate::levels::Tool::Move,
            crate::levels::Tool::Boost,
        ] {
            assert!(level.allows(tool));
        }
        assert_eq!(red_count(&world), 1);
        let radius = world.environment.radius;
        let red_id = world
            .cells
            .iter()
            .find(|cell| cell.color == [1.0, 0.0, 0.0])
            .unwrap()
            .runtime_id;
        let target_x = radius * 0.95 * world.light_angle.cos();
        let target_y = radius * 0.95 * world.light_angle.sin();
        let before_removal = world.cells.len();
        // Remove competitors in the bright patch, then move and boost the
        // native red cell using actions available in this APK challenge.
        compact_cells(&mut world, |_, cell| {
            cell.runtime_id == red_id || (cell.x - target_x).hypot(cell.y - target_y) > 0.25
        });
        assert!(world.cells.len() < before_removal);
        let cell = world
            .cells
            .iter_mut()
            .find(|cell| cell.runtime_id == red_id)
            .unwrap();
        cell.x = target_x;
        cell.y = target_y;
        cell.values[2] = 0.0;
        cell.values[3] = 0.0;
        boost_cell(cell);
        for _ in 0..12_500 {
            step(&mut world, 0.02);
            if red_count(&world) >= 100 {
                let achieved = snapshot(&world, false, 1.0);
                assert!(
                    level
                        .conditions
                        .iter()
                        .all(|condition| crate::levels::condition_met(condition, &achieved))
                );
                eprintln!("breeding reached 100 red after {:.1}h", world.time);
                return;
            }
        }
        panic!(
            "breeding goal not reached: {} red after {:.1} hours",
            red_count(&world),
            world.time,
        );
    }

    #[test]
    #[ignore = "known gap: Coexistence reaches 400 cells but fails to hold for 50 hours"]
    fn coexistence_challenge_can_hold_four_hundred_mixed_cells() {
        let level = crate::levels::bundled()
            .into_iter()
            .find(|level| level.id == "3.5")
            .unwrap();
        let index = bundled::PLATES
            .iter()
            .position(|(name, _)| *name == level.plate)
            .unwrap();
        let mut world = load_bundled(index, true).unwrap();
        assert_eq!(level.place_limit, 1);
        assert!(world.cells.is_empty());
        let mut genes = level.starting_genome();
        genes[0].cell_type = 2;
        genes[1].cell_type = 0;
        genes[2].cell_type = 2;
        genes[0].i = 0;
        genes[0].j = 0;
        genes[1].i = 1;
        genes[1].j = 1;
        genes[2].i = 0;
        genes[2].j = 1;
        genes[0].flags[4] = false;
        genes[2].flags[4] = true;
        for gene in &mut genes[0..3] {
            gene.flags[0] = false;
            gene.floats[0] = 0.30;
        }
        genes[2].flags[0] = true;
        genes[2].floats[2] = (1.5 * std::f64::consts::PI) as f32;
        genes[2].floats[3] = (0.5 * std::f64::consts::PI) as f32;
        genes[2].floats[4] = (0.5 * std::f64::consts::PI) as f32;
        // Send food-feeding descendants inward; the original genome editor
        // permits angles in [0, TAU] and starts cells facing right.
        genes[1].floats[2] = (1.5 * std::f64::consts::PI) as f32;
        genes[1].floats[3] = (0.5 * std::f64::consts::PI) as f32;
        genes[1].floats[4] = (0.5 * std::f64::consts::PI) as f32;
        assert!(level.allows_genes(&genes));
        let x = world.environment.radius * 0.98 * world.light_angle.cos();
        let y = world.environment.radius * 0.98 * world.light_angle.sin();
        world.cells.push(synthesize_cell(x, y, genes).unwrap());
        let mut held_since = None;
        let mut peak = 0;
        let mut peak_time = 0.0;
        let mut longest_hold: f64 = 0.0;
        for step_index in 0..12_500 {
            step(&mut world, 0.02);
            let user_count = world.cells.iter().filter(|cell| cell.e == 1).count();
            if user_count > peak {
                peak = user_count;
                peak_time = world.time;
            }
            if step_index % 1000 == 999 {
                let photocytes = world
                    .cells
                    .iter()
                    .filter(|cell| cell.cell_type() == 2)
                    .count();
                let phagocytes = world
                    .cells
                    .iter()
                    .filter(|cell| cell.cell_type() == 0)
                    .count();
                eprintln!(
                    "coexistence {:.0}h: {user_count} cells ({photocytes} photo, {phagocytes} phago)",
                    world.time
                );
            }
            if user_count >= 400 {
                let since = *held_since.get_or_insert(world.time);
                longest_hold = longest_hold.max(world.time - since);
                if world.time - since >= level.hold_time {
                    let achieved = snapshot(&world, false, 1.0);
                    assert!(
                        level.conditions.iter().all(|condition| {
                            crate::levels::condition_met(condition, &achieved)
                        })
                    );
                    eprintln!(
                        "coexistence reached and held 400 cells after {:.1}h",
                        world.time
                    );
                    return;
                }
            } else {
                held_since = None;
            }
        }
        panic!(
            "coexistence goal not reached: peak {peak} at {peak_time:.1}h, longest hold {longest_hold:.1}h, final {} after {:.1}h",
            world.cells.iter().filter(|cell| cell.e == 1).count(),
            world.time
        );
    }

    #[test]
    fn every_bundled_plate_advances_with_finite_state() {
        assert_eq!(bundled::PLATES.len(), 65);
        for (name, bytes) in bundled::PLATES {
            let mut world = legacy::import(bytes)
                .unwrap_or_else(|error| panic!("{name}: import failed: {error}"));
            let before = world.time;
            for (index, cell) in world.cells.iter().enumerate() {
                assert!(
                    cell.values.iter().all(|value| value.is_finite()),
                    "{name} initial cell {index}: values={:?}",
                    cell.values
                );
            }
            for tick in 0..10 {
                step(&mut world, 0.02);
                for (index, cell) in world.cells.iter().enumerate() {
                    assert!(
                        cell.x.is_finite()
                            && cell.y.is_finite()
                            && cell.values.iter().all(|value| value.is_finite())
                            && cell.u.iter().all(|value| value.is_finite()),
                        "{name} tick {tick} cell {index}: x={} y={} values={:?} u={:?}",
                        cell.x,
                        cell.y,
                        cell.values,
                        cell.u
                    );
                }
            }
            assert!(world.time > before, "{name}: time did not advance");
            assert!(
                world.food.iter().all(|food| {
                    food.x.is_finite() && food.y.is_finite() && food.energy.is_finite()
                }),
                "{name}: non-finite food state"
            );
        }
    }

    #[test]
    fn muscle_contracts_linked_cells() {
        let mut world = legacy::import(bundled::PLATES[0].1).unwrap();
        world.cells.truncate(2);
        for cell in &mut world.cells {
            cell.mode = 0;
            cell.links.clear();
            cell.c = 0;
            cell.values[0] = 0.0;
            cell.values[2] = 0.0;
            cell.values[3] = 0.0;
            cell.values[6] = 0.02;
            cell.values[7] = 0.2;
            cell.y = 0.0;
        }
        world.cells[0].x = 0.0;
        world.cells[1].x = 0.032;
        world.cells[0].genes[0].cell_type = 12;
        world.cells[1].genes[0].cell_type = 0;
        world.cells[0].tail[2] = 0.5;
        let (left, right) = world.cells.split_at_mut(1);
        add_link(&mut left[0], 1, 0.0, std::f64::consts::PI);
        add_link(&mut right[0], 0, std::f64::consts::PI, 0.0);
        solve_links(&mut world, 0.02);
        assert!(world.cells[0].values[2] > 0.01);
        assert!(world.cells[1].values[2] < -0.01);
    }

    #[test]
    fn plate_anchor_damps_motion_and_releases_a_stretched_link() {
        let mut world = legacy::import(bundled::PLATES[0].1).unwrap();
        world.cells.truncate(1);
        let cell = &mut world.cells[0];
        cell.links.clear();
        cell.c = 0;
        cell.x = 0.5;
        cell.y = 0.0;
        cell.values[0] = 0.0;
        cell.values[2] = 0.2;
        cell.values[3] = 0.0;
        cell.values[4] = 0.0;
        cell.values[6] = 0.02;
        cell.values[7] = 0.2;
        add_link(cell, -1, 0.0, 0.0);
        cell.links[0].e = 0.512;
        cell.links[0].f = 0.0;

        solve_links(&mut world, 0.02);
        let damping = 1.0 * (1.0 - 0.02 * 1.0 * 0.5 / 0.2);
        assert!((world.cells[0].values[2] - (0.2 - 0.2 * damping * 0.02 / 0.2)).abs() < 1e-12);
        assert_eq!(world.cells[0].c, 1);

        world.cells[0].x = 0.6;
        solve_links(&mut world, 0.02);
        assert_eq!(world.cells[0].c, 0);
    }

    #[test]
    fn cilia_push_on_neighbor_contact() {
        let mut world = legacy::import(bundled::PLATES[0].1).unwrap();
        world.cells.truncate(2);
        for cell in &mut world.cells {
            cell.mode = 0;
            cell.links.clear();
            cell.c = 0;
            cell.values[2..=5].fill(0.0);
            cell.values[6] = 0.02;
            cell.values[7] = 0.2;
            cell.y = 0.0;
        }
        world.cells[0].x = 0.0;
        world.cells[1].x = 0.03;
        world.cells[0].values[0] = -std::f64::consts::FRAC_PI_2;
        world.cells[0].genes[0].cell_type = 17;
        world.cells[1].genes[0].cell_type = 0;
        // The APK uses output 2 on the forward-facing side of this contact.
        world.cells[0].genes[0].inputs[2] = GeneInput {
            source: 0,
            mode: 0,
            a: 0.0,
            b: 1.0,
            c: 0.0,
        };
        let mut contacts = solve_contacts(&mut world, 0.02, false, &mut rand::rng());
        assert_eq!(contacts, vec![(0, Some(1))]);
        apply_ciliary_contacts(&mut world, &mut contacts);
        assert!(world.cells[0].values[3] > 0.005);
        assert!(world.cells[1].values[3] < -0.005);
        let inertia = 0.7 * 0.2 * 0.02_f64.powi(2);
        let arm = 0.015;
        let impulse = 0.075 * 0.9 / (2.0 / 0.2 + 2.0 * arm * arm / inertia);
        assert!((world.cells[0].values[3] - impulse / 0.2).abs() < 1.0e-12);
        assert!((world.cells[0].values[4] - arm * impulse / inertia).abs() < 1.0e-12);
        let tangent_momentum = world.cells[0].values[3] * world.cells[0].values[7]
            + world.cells[1].values[3] * world.cells[1].values[7];
        assert!(tangent_momentum.abs() < 1.0e-12);
    }

    #[test]
    fn cilia_push_against_the_plate_after_normal_pressure() {
        let mut world = legacy::import(bundled::PLATES[0].1).unwrap();
        world.cells.truncate(1);
        world.environment.x = false;
        let cell = &mut world.cells[0];
        cell.mode = 0;
        cell.genes[0].cell_type = 17;
        cell.genes[0].inputs[2] = GeneInput {
            source: 0,
            mode: 0,
            a: 0.0,
            b: 1.0,
            c: 0.0,
        };
        cell.x = world.environment.radius - 0.01;
        cell.y = 0.0;
        cell.values[0] = -std::f64::consts::FRAC_PI_2;
        cell.values[2..=5].fill(0.0);
        cell.values[6] = 0.02;
        cell.values[7] = 0.2;
        cell.x_flag = false;
        cell.links.clear();
        cell.c = 0;

        let mut contacts = apply_plate_edge_force(&mut world, 0.02);
        assert_eq!(contacts, vec![(0, None)]);
        assert_eq!(world.cells[0].values[3], 0.0);
        apply_ciliary_contacts(&mut world, &mut contacts);
        assert!(world.cells[0].values[3] > 0.0);
        assert!(world.cells[0].values[4] > 0.0);
        let inertia = 0.7 * 0.2 * 0.02_f64.powi(2);
        let impulse = 0.075 * 0.9 / (1.0 / 0.2 + 0.01_f64.powi(2) / inertia);
        assert!((world.cells[0].values[3] - impulse / 0.2).abs() < 1.0e-12);
        assert!((world.cells[0].values[4] - 0.01 * impulse / inertia).abs() < 1.0e-12);
    }

    #[test]
    fn secrocyte_activates_food_for_a_phagocyte() {
        let mut world = legacy::import(bundled::PLATES[0].1).unwrap();
        world.cells.truncate(2);
        world.food.truncate(1);
        world.environment.y = false;
        for cell in &mut world.cells {
            cell.mode = 0;
            cell.x = 0.0;
            cell.y = 0.0;
            cell.values[6] = 0.02;
            cell.values[7] = 0.2;
        }
        world.cells[0].genes[0].cell_type = 14;
        world.cells[0].genes[0].output_modes[4] = 2;
        world.cells[1].genes[0].cell_type = 0;
        world.food[0].x = 0.0;
        world.food[0].y = 0.0;
        world.food[0].energy = 0.1;
        world.food[0].state = 0.01;

        eat_food(&mut world, 0.02);
        assert!(world.food[0].state <= 0.0);
        assert!((world.cells[1].values[7] - 0.22).abs() < 1.0e-6);
        assert!((world.food[0].energy - 0.08).abs() < 1.0e-6);
    }

    #[test]
    fn crowded_plate_food_grid_reaches_a_large_phagocyte() {
        let mut world = legacy::import(bundled::PLATES[0].1).unwrap();
        world.environment.radius = 0.15;
        world.environment.y = false;
        let mut phagocyte = world.cells[0].clone();
        phagocyte.mode = 0;
        phagocyte.genes[0].cell_type = 0;
        phagocyte.x = -0.06;
        phagocyte.y = 0.0;
        phagocyte.values[6] = 0.08;
        phagocyte.values[7] = 0.2;
        world.cells = vec![phagocyte.clone()];
        for index in 0..40 {
            let mut distant = phagocyte.clone();
            distant.y = 10.0 + index as f64;
            world.cells.push(distant);
        }
        world.food.truncate(1);
        world.food[0].x = 0.02;
        world.food[0].y = 0.0;
        world.food[0].energy = 0.1;
        world.food[0].state = 0.0;
        eat_food(&mut world, 0.02);
        assert!((world.cells[0].values[7] - 0.22).abs() < 1.0e-6);
        assert!((world.food[0].energy - 0.08).abs() < 1.0e-6);
    }

    #[test]
    fn cilia_push_moving_food_and_feeding_conserves_cell_momentum() {
        let mut world = legacy::import(bundled::PLATES[0].1).unwrap();
        world.cells.truncate(1);
        world.food.truncate(1);
        world.environment.y = true;
        world.environment.g = 0.0;
        let cell = &mut world.cells[0];
        cell.mode = 0;
        cell.genes[0].cell_type = 17;
        cell.genes[0].inputs[2] = GeneInput {
            source: 0,
            mode: 0,
            a: 0.0,
            b: 1.0,
            c: 0.0,
        };
        cell.x = 0.0;
        cell.y = 0.0;
        cell.values[0] = -std::f64::consts::FRAC_PI_2;
        cell.y_flag = false;
        cell.values[2] = 0.0;
        cell.values[3] = 0.0;
        cell.values[4] = 0.0;
        cell.values[6] = 0.02;
        world.food[0].x = 0.025;
        world.food[0].y = 0.0;
        world.food[0].vx = 0.0;
        world.food[0].vy = 0.0;
        world.food[0].energy = 0.1;
        eat_food(&mut world, 0.02);
        assert!(world.food[0].vy < -0.05, "vy={}", world.food[0].vy);

        world.environment.y = false;
        world.cells[0].genes[0].cell_type = 0;
        world.cells[0].values[2] = 0.1;
        world.cells[0].values[7] = 0.2;
        world.food[0].x = 0.0;
        world.food[0].y = 0.0;
        world.food[0].energy = 0.1;
        world.food[0].state = 0.0;
        eat_food(&mut world, 0.02);
        assert!((world.cells[0].values[7] - 0.22).abs() < 1.0e-6);
        assert!((world.cells[0].values[2] - 0.1 * 0.2 / 0.22).abs() < 1.0e-6);
    }

    #[test]
    fn division_respects_link_limit_and_splits_lipocyte_reserves() {
        let mut world = legacy::import(bundled::PLATES[0].1).unwrap();
        world.cells.truncate(1);
        world.environment.max_cells = 2;
        let parent = &mut world.cells[0];
        parent.mode = 0;
        parent.genes[0].cell_type = 4;
        parent.genes[0].floats[0] = 0.1;
        parent.genes[0].floats[1] = 0.3;
        parent.genes[0].i = 0;
        parent.genes[0].j = 0;
        parent.genes[0].flags[0] = true;
        parent.genes[0].output_modes[9] = 0;
        parent.values[7] = 0.36;
        parent.values[8] = 1.0;
        parent.tuvp[3] = 1.0;
        parent.tail[5] = 0.1;
        parent.u[0] = 0.8;
        parent.c = 1;
        reproduce(&mut world);
        assert_eq!(world.cells.len(), 1);

        world.cells[0].c = 0;
        world.cells[0].f = 101;
        world.cells[0].mutation_ticks = 8;
        reproduce(&mut world);
        assert_eq!(world.cells.len(), 2);
        assert_eq!(world.cells[0].mutation_ticks, 0);
        assert_eq!(world.cells[1].mutation_ticks, 0);
        assert_eq!(world.cells[1].f, 0);
        assert!((world.cells[0].values[7] + world.cells[1].values[7] - 0.46).abs() < 1.0e-6);
        assert_eq!(world.cells[0].values[8], 0.0);
        assert_eq!(world.cells[1].values[8], 0.0);
        assert_eq!(world.cells[0].tuvp[3], 0.5);
        assert_eq!(world.cells[1].tuvp[3], 0.5);
        assert_eq!(world.cells[0].links[0].target, 1);
        assert_eq!(world.cells[1].links[0].target, 0);
        assert!((world.cells[0].u[0] - 0.2401).abs() < 1.0e-5);
    }

    #[test]
    fn division_only_spends_mass_on_extra_armor() {
        let mut world = legacy::import(bundled::PLATES[0].1).unwrap();
        world.cells.truncate(1);
        world.environment.max_cells = 2;
        let parent = &mut world.cells[0];
        parent.mode = 0;
        parent.genes[0].cell_type = 0;
        parent.genes[0].floats[0] = 0.1;
        parent.genes[0].floats[1] = 0.5;
        parent.genes[0].i = 0;
        parent.genes[0].j = 0;
        parent.genes[0].output_values[6] = 1.0;
        parent.values[7] = 0.36;
        parent.values[8] = 1.0;
        parent.tuvp[3] = 1.0;
        parent.x_flag = false;
        parent.links.clear();
        parent.c = 0;
        let mut armored = world.clone();
        armored.cells[0].genes[0].output_values[6] = 2.0;

        reproduce(&mut world);
        reproduce(&mut armored);
        assert_eq!(world.cells.len(), 2);
        assert_eq!(armored.cells.len(), 2);
        let ordinary_mass: f64 = world.cells.iter().map(|cell| cell.values[7]).sum();
        let armored_mass: f64 = armored.cells.iter().map(|cell| cell.values[7]).sum();
        assert!((ordinary_mass - 0.36).abs() < 1.0e-8);
        assert!((armored_mass - (0.36 - 2.0 * 0.054000005)).abs() < 1.0e-8);
    }

    #[test]
    fn new_daughter_can_feed_during_the_division_step() {
        let mut world = legacy::import(bundled::PLATES[0].1).unwrap();
        world.cells.truncate(1);
        world.food.truncate(1);
        world.environment.a = 0.0;
        world.environment.w = false;
        world.environment.y = false;
        world.environment.max_cells = 2;
        let cell = &mut world.cells[0];
        cell.mode = 0;
        cell.genes[0].cell_type = 0;
        cell.genes[0].floats[0] = 0.1;
        cell.genes[0].floats[1] = 0.5;
        cell.genes[0].i = 0;
        cell.genes[0].j = 0;
        cell.values[2] = 0.0;
        cell.values[3] = 0.0;
        cell.values[7] = 0.36;
        cell.values[8] = 1.0;
        cell.tuvp[3] = 1.0;
        cell.x = 0.0;
        cell.y = 0.0;
        cell.links.clear();
        cell.c = 0;
        world.food[0].x = 0.0;
        world.food[0].y = 0.0;
        world.food[0].energy = 0.5;
        world.food[0].state = 0.0;
        let mut without_food = world.clone();
        without_food.food.clear();

        step(&mut world, 0.02);
        step(&mut without_food, 0.02);
        assert_eq!(world.cells.len(), 2);
        assert_eq!(without_food.cells.len(), 2);
        let with_mass: f64 = world.cells.iter().map(|cell| cell.values[7]).sum();
        let without_mass: f64 = without_food.cells.iter().map(|cell| cell.values[7]).sum();
        assert!((with_mass - without_mass - 0.04).abs() < 1.0e-6);
    }

    #[test]
    fn division_uses_age_from_the_previous_metabolism_pass() {
        let mut world = legacy::import(bundled::PLATES[0].1).unwrap();
        world.cells.truncate(1);
        world.food.clear();
        world.environment.a = 0.0;
        world.environment.w = false;
        world.environment.max_cells = 2;
        let cell = &mut world.cells[0];
        cell.mode = 0;
        cell.genes[0].cell_type = 0;
        cell.genes[0].floats[0] = 0.1;
        cell.genes[0].floats[1] = 0.5;
        cell.genes[0].i = 0;
        cell.genes[0].j = 0;
        cell.x = 0.0;
        cell.y = 0.0;
        cell.values[2] = 0.0;
        cell.values[3] = 0.0;
        cell.values[7] = 0.36;
        cell.values[8] = 0.49;
        cell.tuvp[3] = 1.0;
        cell.links.clear();
        cell.c = 0;

        step(&mut world, 0.02);
        assert_eq!(world.cells.len(), 1);
        assert!(world.cells[0].values[8] > 0.5);
        step(&mut world, 0.02);
        assert_eq!(world.cells.len(), 2);
    }

    #[test]
    fn lipocyte_daughters_store_mass_above_the_body_cap() {
        let mut world = legacy::import(bundled::PLATES[0].1).unwrap();
        world.cells.truncate(1);
        world.environment.max_cells = 2;
        let parent = &mut world.cells[0];
        parent.mode = 0;
        parent.genes[0].cell_type = 4;
        parent.genes[0].floats[0] = 0.1;
        parent.genes[0].floats[1] = 0.5;
        parent.genes[0].i = 0;
        parent.genes[0].j = 0;
        parent.values[7] = 0.36;
        parent.values[8] = 1.0;
        parent.tail[5] = 1.8;
        parent.tuvp[3] = 1.0;
        parent.x_flag = false;
        parent.links.clear();
        parent.c = 0;

        reproduce(&mut world);
        assert_eq!(world.cells.len(), 2);
        for cell in &world.cells {
            assert!((cell.values[7] - 0.36).abs() < 1.0e-8);
            assert!((cell.tail[5] as f64 - 0.72).abs() < 1.0e-6);
        }
    }

    #[test]
    fn division_marks_flagellocyte_with_a_rear_link_for_removal() {
        let mut world = legacy::import(bundled::PLATES[0].1).unwrap();
        world.cells.truncate(1);
        world.food.clear();
        world.environment.a = 0.0;
        world.environment.w = false;
        world.environment.max_cells = 2;
        let parent = &mut world.cells[0];
        parent.mode = 0;
        parent.genes[0].cell_type = 1;
        parent.genes[0].flags[0] = true;
        parent.genes[0].floats[0] = 0.1;
        parent.genes[0].floats[1] = 0.5;
        parent.genes[0].floats[2..=4].fill(0.0);
        parent.genes[0].i = 0;
        parent.genes[0].j = 0;
        parent.values[0] = 0.0;
        parent.values[2..=5].fill(0.0);
        parent.values[7] = 0.36;
        parent.values[8] = 1.0;
        parent.x = 0.0;
        parent.y = 0.0;
        parent.tuvp[3] = 1.0;
        parent.x_flag = false;
        parent.links.clear();
        parent.c = 0;

        let mut stepped = world.clone();
        reproduce(&mut world);
        assert_eq!(world.cells.len(), 2);
        assert!(!world.cells[0].x_flag);
        assert!(world.cells[1].x_flag);
        assert_eq!(world.cells[0].links[0].target, 1);
        assert_eq!(world.cells[1].links[0].target, 0);

        step(&mut stepped, 0.02);
        assert_eq!(stepped.cells.len(), 1);
        assert_eq!(stepped.cells[0].c, 0);
    }

    #[test]
    fn division_passes_existing_links_to_the_correct_side() {
        let mut world = legacy::import(bundled::PLATES[0].1).unwrap();
        world.cells.truncate(3);
        world.environment.max_cells = 4;
        let link = |target, anchor| Link {
            target,
            c: anchor,
            d: 0.0,
            b: true,
            e: 0.0,
            f: 0.0,
            k: 0.0,
            l: 0.0,
        };
        for neighbor in &mut world.cells[1..] {
            neighbor.values[8] = 0.0;
        }
        world.cells[1].links = vec![link(0, 0.0)];
        world.cells[1].c = 1;
        world.cells[2].links = vec![link(0, 0.0)];
        world.cells[2].c = 1;
        let parent = &mut world.cells[0];
        parent.links = vec![link(1, 0.0), link(2, std::f64::consts::PI)];
        parent.c = 2;
        parent.mode = 0;
        parent.genes[0].cell_type = 0;
        parent.genes[0].floats[0] = 0.1;
        parent.genes[0].floats[1] = 0.5;
        parent.genes[0].floats[2] = 0.0;
        parent.genes[0].i = 0;
        parent.genes[0].j = 0;
        parent.genes[0].flags[0] = false;
        parent.genes[0].flags[1] = true;
        parent.genes[0].flags[2] = true;
        parent.genes[0].output_modes[9] = 19;
        parent.values[0] = 0.0;
        parent.values[7] = 0.36;
        parent.values[8] = 1.0;
        parent.tuvp[3] = 1.0;
        reproduce(&mut world);
        assert_eq!(world.cells.len(), 4);
        assert_eq!(
            world.cells[0]
                .links
                .iter()
                .map(|link| link.target)
                .collect::<Vec<_>>(),
            vec![2]
        );
        assert_eq!(
            world.cells[3]
                .links
                .iter()
                .map(|link| link.target)
                .collect::<Vec<_>>(),
            vec![1]
        );
        assert_eq!(world.cells[1].links[0].target, 3);
        assert_eq!(world.cells[2].links[0].target, 0);
        assert_eq!(world.cells[0].c, 1);
        assert_eq!(world.cells[3].c, 1);
    }

    #[test]
    fn simultaneous_division_preserves_links_between_new_sides() {
        let mut world = legacy::import(bundled::PLATES[0].1).unwrap();
        world.cells.truncate(2);
        world.environment.max_cells = 4;
        for cell in &mut world.cells {
            cell.mode = 0;
            cell.genes[0].cell_type = 0;
            cell.genes[0].floats[0] = 0.1;
            cell.genes[0].floats[1] = 0.5;
            cell.genes[0].floats[2] = 0.0;
            cell.genes[0].i = 0;
            cell.genes[0].j = 0;
            cell.genes[0].flags[0] = false;
            cell.genes[0].flags[1] = true;
            cell.genes[0].flags[2] = true;
            cell.genes[0].output_modes[9] = 19;
            cell.values[0] = 0.0;
            cell.values[7] = 0.36;
            cell.values[8] = 1.0;
            cell.tuvp[3] = 1.0;
            cell.c = 1;
        }
        let link = |target, anchor| Link {
            target,
            c: anchor,
            d: 0.0,
            b: true,
            e: 0.0,
            f: 0.0,
            k: 0.0,
            l: 0.0,
        };
        world.cells[0].links = vec![link(1, 0.0)];
        world.cells[1].links = vec![link(0, std::f64::consts::PI)];
        reproduce(&mut world);
        assert_eq!(world.cells.len(), 4);
        assert_eq!(world.cells[2].links[0].target, 1);
        assert_eq!(world.cells[1].links[0].target, 2);
        assert_eq!(world.cells[2].c, 1);
        assert_eq!(world.cells[1].c, 1);
    }

    #[test]
    fn loaded_plate_advances_with_resized_limits() {
        let mut world = legacy::import(bundled::PLATES[0].1).unwrap();
        let start = world.time;
        world.set_max_cells(world.cells.len() + 10);
        world.set_max_food(world.food.len() + 10);
        world.set_radius(1.5);
        for _ in 0..100 {
            step(&mut world, 0.02);
        }
        assert!(world.time > start);
        assert!(
            !world.cells.is_empty(),
            "imported organisms disappeared during a short simulation run"
        );
        assert!(world.cells.len() <= world.environment.max_cells);
        assert!(world.food.len() <= world.environment.max_food);
        assert!(
            world
                .cells
                .iter()
                .all(|cell| cell.x.is_finite() && cell.y.is_finite())
        );
    }

    #[test]
    fn removing_a_cell_reindexes_links() {
        let mut world = legacy::import(bundled::PLATES[0].1).unwrap();
        world.cells.truncate(3);
        world.cells[0].links[0].target = 2;
        compact_cells(&mut world, |i, _| i != 1);
        assert_eq!(world.cells.len(), 2);
        assert_eq!(world.cells[0].links[0].target, 1);
    }

    #[test]
    fn compaction_preserves_active_link_order_and_dormant_slots() {
        let mut world = legacy::import(bundled::PLATES[0].1).unwrap();
        world.cells.truncate(3);
        let template = world.cells[0].links[0].clone();
        world.cells[0].links = [1, 2, -1, 99]
            .into_iter()
            .map(|target| Link {
                target,
                ..template.clone()
            })
            .collect();
        world.cells[0].c = 3;
        let capacity = world.cells[0].links.capacity();
        compact_cells(&mut world, |index, _| index != 1);
        assert_eq!(world.cells[0].c, 2);
        assert_eq!(
            world.cells[0]
                .links
                .iter()
                .map(|link| link.target)
                .collect::<Vec<_>>(),
            vec![1, -1, 99]
        );
        assert_eq!(world.cells[0].links.capacity(), capacity);
    }

    #[test]
    fn removed_neighbors_drop_links_but_plate_anchors_remain_visible() {
        let mut world = legacy::import(bundled::PLATES[0].1).unwrap();
        world.cells.truncate(2);
        world.cells[0].x = 0.1;
        world.cells[0].y = 0.0;
        world.cells[0].values[0] = 0.0;
        world.cells[0].values[6] = 0.03;
        world.cells[0].links[0].target = 1;
        world.cells[0].links[1].target = -1;
        world.cells[0].links[1].c = 0.0;
        world.cells[0].links[1].e = 0.25;
        world.cells[0].links[1].f = -0.125;
        world.cells[0].c = 2;
        compact_cells(&mut world, |index, _| index == 0);
        assert_eq!(world.cells[0].c, 1);
        assert_eq!(world.cells[0].links[0].target, -1);
        let render = snapshot(&world, false, 1.0);
        assert_eq!(render.links.len(), 1);
        assert!((render.links[0].from[0] - 0.113_266_875).abs() < 0.000_001);
        assert_eq!(render.links[0].from[1], 0.0);
        assert_eq!(render.links[0].to, [0.25, -0.125]);
        assert!((render.links[0].left[0] - 0.175).abs() < 0.000_001);
        assert!((render.links[0].right[0] - 0.175).abs() < 0.000_001);
        assert!(render.links[0].left[1] < -0.0625);
        assert!(render.links[0].right[1] > -0.0625);
    }

    #[test]
    fn link_bar_uses_apk_radius_weighted_center() {
        let mut world = legacy::import(bundled::PLATES[0].1).unwrap();
        world.cells.truncate(2);
        world.cells[0].x = 0.0;
        world.cells[0].y = 0.0;
        world.cells[0].values[6] = 0.03;
        world.cells[0].c = 0;
        world.cells[1].x = 0.1;
        world.cells[1].y = 0.0;
        world.cells[1].values[6] = 0.02;
        world.cells[1].values[0] = 0.0;
        world.cells[1].links[0].target = 0;
        world.cells[1].links[0].c = std::f64::consts::PI;
        world.cells[1].links[0].d = 0.0;
        world.cells[1].c = 1;
        let render = snapshot(&world, false, 1.0);
        assert_eq!(render.links.len(), 1);
        assert!((render.links[0].left[0] - 0.045).abs() < 0.000_001);
        assert!((render.links[0].right[0] - 0.045).abs() < 0.000_001);
        assert!(render.links[0].left[1] < 0.0);
        assert!(render.links[0].right[1] > 0.0);
    }

    #[test]
    fn myocyte_bend_moves_both_link_attachments_without_moving_bar() {
        let mut world = legacy::import(bundled::PLATES[0].1).unwrap();
        world.cells.truncate(2);
        world.cells[0].x = 0.0;
        world.cells[0].y = 0.0;
        world.cells[0].values[0] = 0.0;
        world.cells[0].values[6] = 0.03;
        world.cells[1].x = 0.1;
        world.cells[1].y = 0.0;
        world.cells[1].values[0] = 0.0;
        world.cells[1].values[6] = 0.02;
        let mut link = world.cells[1].links[0].clone();
        link.target = 0;
        link.c = std::f64::consts::PI;
        link.d = 0.0;
        let baseline = render_link_geometry(&world.cells[1], &link, Some(&world.cells[0])).unwrap();
        world.cells[0].mode = 0;
        world.cells[1].mode = 0;
        world.cells[0].genes[0].cell_type = 12;
        world.cells[1].genes[0].cell_type = 12;
        world.cells[0].tail[3] = 0.25;
        world.cells[1].tail[3] = 0.5;
        let bent = render_link_geometry(&world.cells[1], &link, Some(&world.cells[0])).unwrap();
        let source_inset = (0.02 - 0.00018_f64.sqrt()) * 0.8;
        let target_inset = (0.03 - 0.00018_f64.sqrt()) * 0.8;
        assert!(
            (bent.from[1] as f64 - source_inset * (std::f64::consts::PI - 0.5).sin()).abs()
                < 1.0e-6
        );
        assert!((bent.to[1] as f64 - target_inset * 0.25_f64.sin()).abs() < 1.0e-6);
        assert!(bent.from[1] > baseline.from[1]);
        assert!(bent.to[1] > baseline.to[1]);
        assert_eq!(bent.left, baseline.left);
        assert_eq!(bent.right, baseline.right);
    }

    #[test]
    fn removed_cells_make_bounded_fading_particles() {
        let mut world = legacy::import(bundled::PLATES[0].1).unwrap();
        world.cells.truncate(2);
        let mut removed_on_step = world.cells[0].clone();
        world.environment.max_cells = 4;
        for cell in &mut world.cells {
            cell.x = 0.0;
            cell.y = 0.0;
            cell.values[6] = 0.03;
        }
        compact_cells(&mut world, |_, _| false);
        assert_eq!(world.dead.len(), 1, "APK limit is one quarter of max cells");
        assert_eq!(world.dead[0].radius, 0.03);
        world.dead[0].age = 0.95;
        step(&mut world, 0.2);
        assert!(world.dead.is_empty(), "expired particles should be removed");
        removed_on_step.x_flag = true;
        world.cells.push(removed_on_step);
        world.environment.w = false;
        step(&mut world, 0.02);
        assert_eq!(world.dead.len(), 1);
        assert!((world.dead[0].age - 0.01).abs() < 1.0e-6);
    }

    #[test]
    fn natural_death_recycles_mass_but_manual_removal_does_not() {
        let mut world = legacy::import(bundled::PLATES[0].1).unwrap();
        world.cells.truncate(1);
        world.food.clear();
        world.environment.a = 0.0;
        world.environment.w = false;
        world.environment.max_food = 10;
        let cell = &mut world.cells[0];
        cell.mode = 0;
        cell.genes[0].cell_type = 4;
        cell.x = 0.0;
        cell.y = 0.0;
        cell.values[7] = 0.2;
        cell.x_flag = true;
        step(&mut world, 0.02);
        assert!(world.cells.is_empty());
        assert_eq!(world.food.len(), 1);
        assert!((world.food[0].energy - (0.18 - 0.006 * 0.02)).abs() < 1.0e-6);
        assert_eq!(world.food[0].state, 0.5);

        let mut manually_removed = legacy::import(bundled::PLATES[0].1).unwrap();
        manually_removed.cells.truncate(1);
        manually_removed.food.clear();
        compact_cells(&mut manually_removed, |_, _| false);
        assert!(manually_removed.food.is_empty());
    }

    #[test]
    fn recycled_food_can_feed_a_survivor_in_the_same_step() {
        let mut world = legacy::import(bundled::PLATES[0].1).unwrap();
        world.cells.truncate(2);
        world.food.clear();
        world.environment.a = 0.0;
        world.environment.w = false;
        world.environment.max_food = 10;
        for cell in &mut world.cells {
            cell.mode = 0;
            cell.genes[0].cell_type = 0;
            cell.x = 0.0;
            cell.y = 0.0;
            cell.values[7] = 0.2;
            cell.values[8] = 0.0;
        }
        world.cells[0].x_flag = true;
        step(&mut world, 0.02);
        assert_eq!(world.cells.len(), 1);
        assert_eq!(world.food.len(), 1);
        assert!(world.cells[0].values[7] > 0.215);
        assert!(world.food[0].energy < 0.161);
    }

    #[test]
    fn boundary_and_contacts_keep_cells_in_plate() {
        let mut world = legacy::import(bundled::PLATES[0].1).unwrap();
        world.cells.truncate(2);
        world.environment.a = 0.0;
        world.environment.x = false;
        for cell in &mut world.cells {
            cell.links.clear();
            cell.c = 0;
            cell.x_flag = false;
            cell.values[6] = 0.02;
            cell.values[7] = 0.2;
            cell.values[8] = 0.0;
            cell.values[2] = 0.0;
            cell.values[3] = 0.0;
        }
        world.cells[0].x = 0.0;
        world.cells[0].y = 0.0;
        world.cells[1].x = 0.02;
        world.cells[1].y = 0.0;
        step(&mut world, 0.02);
        assert_eq!(world.cells.len(), 2);
        assert!(world.cells[0].values[2] < 0.0);
        assert!(world.cells[1].values[2] > 0.0);
        step(&mut world, 0.02);
        assert!(world.cells[1].x - world.cells[0].x > 0.02);
        world.cells[0].x = world.environment.radius - 0.03;
        world.cells[0].y = 0.0;
        world.cells[0].values[2] = 0.75;
        step(&mut world, 0.02);
        assert_eq!(world.cells.len(), 2);
        assert!(world.cells[0].x > world.environment.radius - world.cells[0].values[6]);
        assert!(world.cells[0].x < world.environment.radius);
        assert!(world.cells[0].values[2] < 0.75);
    }

    #[test]
    fn apk_food_edge_respects_motion_and_removal_flags() {
        let mut world = legacy::import(bundled::PLATES[0].1).unwrap();
        world.cells.clear();
        world.food.clear();
        world.environment.a = 0.0;
        world.environment.w = false;
        world.environment.g = 0.0;
        world.environment.radius = 0.5;
        world.environment.x = true;
        world.environment.y = false;
        world.food.push(crate::model::Food {
            x: 0.6,
            y: 0.0,
            energy: 1.0,
            vx: 1.0,
            vy: 0.0,
            state: 0.0,
        });
        step(&mut world, 0.02);
        assert_eq!(
            world.food.len(),
            1,
            "stationary food stays after shrinking a plate"
        );
        assert_eq!(world.food[0].x, 0.6);

        world.environment.y = true;
        world.environment.x = false;
        step(&mut world, 0.02);
        assert_eq!(world.food.len(), 1);
        assert!((world.food[0].x - 0.5).abs() < 1.0e-6);
        assert!(world.food[0].vx < 0.0, "moving food reflects at the rim");

        world.food[0].x = 0.6;
        world.environment.x = true;
        step(&mut world, 0.02);
        assert!(
            world.food.is_empty(),
            "moving food can leave a permeable plate"
        );
    }

    #[test]
    fn apk_cell_contact_pressure_changes_velocity_and_preserves_momentum() {
        let mut world = legacy::import(bundled::PLATES[0].1).unwrap();
        let mut first = world.cells[0].clone();
        first.mode = 0;
        first.genes[0].cell_type = 0;
        first.genes[0].output_values[6] = 1.0;
        first.x = 0.0;
        first.y = 0.0;
        first.values[2] = 0.0;
        first.values[3] = 0.0;
        first.values[6] = 0.02;
        first.values[7] = 0.2;
        first.x_flag = false;
        first.links.clear();
        first.c = 0;
        let mut second = first.clone();
        second.x = 0.03;
        world.cells = vec![first, second];
        solve_contacts(&mut world, 0.02, false, &mut rand::rng());
        // The APK's overlap chord is 0.0007 at this spacing, giving
        // force 0.35 and a velocity change of 0.35 * 0.02 / 0.2.
        assert!((world.cells[0].values[2] + 0.035).abs() < 1e-4);
        assert!((world.cells[1].values[2] - 0.035).abs() < 1e-4);
        assert_eq!(world.cells[0].x, 0.0);
        assert_eq!(world.cells[1].x, 0.03);

        world.cells[0].values[2] = 0.0;
        world.cells[1].values[2] = 0.0;
        let (left, right) = world.cells.split_at_mut(1);
        add_link(&mut left[0], 1, 0.0, std::f64::consts::PI);
        add_link(&mut right[0], 0, std::f64::consts::PI, 0.0);
        solve_contacts(&mut world, 0.02, false, &mut rand::rng());
        assert!((world.cells[0].values[2] + 0.035).abs() < 1e-4);
        assert!((world.cells[1].values[2] - 0.035).abs() < 1e-4);
    }

    #[test]
    fn contact_pressure_stays_finite_for_almost_coincident_centers() {
        let world = legacy::import(bundled::PLATES[0].1).unwrap();
        let mut first = world.cells[0].clone();
        first.mode = 0;
        first.genes[0].cell_type = 0;
        first.genes[0].output_values[6] = 1.0;
        first.x = 0.0;
        first.y = 0.0;
        first.values[6] = 0.02;
        let mut second = first.clone();
        second.x = 1.0e-150;
        let squared = (second.x - first.x).powi(2);
        let (fx, fy) = cell_contact_force(&first, &second, squared).unwrap();
        assert!(fx.is_finite() && fy.is_finite());
        assert!(fx < 0.0);
    }

    #[test]
    fn infected_cell_uses_its_new_gene_in_later_contacts() {
        let mut world = legacy::import(bundled::PLATES[0].1).unwrap();
        let mut virus = world.cells[0].clone();
        virus.mode = 0;
        virus.genes[0].cell_type = 8;
        virus.x = 0.0;
        virus.y = 0.0;
        virus.values[6] = 0.02;
        virus.values[7] = 0.2;
        virus.values[8] = 1.0;
        virus.x_flag = false;
        virus.links.clear();
        virus.c = 0;
        let mut middle = virus.clone();
        middle.genes[0].cell_type = 0;
        middle.x = 0.025;
        let mut last = middle.clone();
        last.x = 0.05;
        world.cells = vec![virus, middle, last];
        world.environment.radius = 1.0;
        solve_contacts(&mut world, 1.0, false, &mut rand::rng());
        assert_eq!(world.cells[1].genes[0].cell_type, 8);
        assert_eq!(world.cells[2].genes[0].cell_type, 8);
    }

    #[test]
    fn crowded_plate_contact_grid_detects_overlapping_large_cells() {
        let mut world = legacy::import(bundled::PLATES[0].1).unwrap();
        world.environment.radius = 0.15;
        let mut first = world.cells[0].clone();
        first.mode = 0;
        first.genes[0].cell_type = 0;
        first.x = -0.06;
        first.y = 0.0;
        first.values[2] = 0.0;
        first.values[3] = 0.0;
        first.values[6] = 0.08;
        first.values[7] = 0.2;
        first.x_flag = false;
        first.links.clear();
        first.c = 0;
        let mut second = first.clone();
        second.x = 0.07;
        world.cells = vec![first.clone(), second];
        for index in 0..24 {
            let mut distant = first.clone();
            distant.y = 10.0 + index as f64;
            distant.x_flag = true;
            world.cells.push(distant);
        }
        solve_contacts(&mut world, 0.02, false, &mut rand::rng());
        assert!(world.cells[0].values[2] < 0.0);
        assert!(world.cells[1].values[2] > 0.0);
    }

    #[test]
    fn apk_plate_pressure_and_glueocyte_anchor_act_at_the_edge() {
        let mut world = legacy::import(bundled::PLATES[0].1).unwrap();
        world.cells.truncate(1);
        world.environment.radius = 1.0;
        world.environment.x = false;
        let cell = &mut world.cells[0];
        cell.mode = 0;
        cell.genes[0].cell_type = 7;
        cell.genes[0].output_values[6] = 1.0;
        cell.x = 0.99;
        cell.y = 0.0;
        cell.values[0] = 0.0;
        cell.values[2] = 0.0;
        cell.values[3] = 0.0;
        cell.values[6] = 0.02;
        cell.values[7] = 0.2;
        cell.links.clear();
        cell.c = 0;
        cell.x_flag = false;
        apply_plate_edge_force(&mut world, 0.02);
        let distance = 0.99;
        let chord = ((-distance + 0.02 - 1.0)
            * (-distance - 0.02 + 1.0)
            * (-distance + 0.02 + 1.0)
            * (distance + 0.02 + 1.0))
            / distance;
        let expected_velocity = -1000.0 * chord * 0.02 / 0.2;
        assert!((world.cells[0].values[2] - expected_velocity).abs() < 1e-12);
        assert_eq!(world.cells[0].x, 0.99);
        assert_eq!(world.cells[0].c, 1);
        assert_eq!(world.cells[0].links[0].target, -1);
        assert!((world.cells[0].links[0].e - 1.005_366_563_145_999_5).abs() < 1e-12);
        apply_plate_edge_force(&mut world, 0.02);
        assert_eq!(world.cells[0].c, 1);
    }

    #[test]
    fn apk_fluid_drag_slows_free_cells_with_zero_surface_friction() {
        let mut world = legacy::import(bundled::PLATES[0].1).unwrap();
        world.cells.truncate(1);
        world.food.clear();
        world.environment.max_food = 0;
        world.environment.w = false;
        world.environment.i = 0.0;
        world.environment.m = 0.0;
        world.environment.g = 60.0;
        let cell = &mut world.cells[0];
        cell.mode = 0;
        cell.genes[0].cell_type = 0;
        cell.x = 0.0;
        cell.y = 0.0;
        cell.values[2] = 0.2;
        cell.values[3] = 0.1;
        cell.values[4] = 0.5;
        cell.values[6] = (0.288_f64 / 400.0).sqrt();
        cell.values[7] = 0.288;
        cell.values[8] = 0.0;
        cell.tuvp[3] = 1.0;
        cell.x_flag = false;
        cell.links.clear();
        cell.c = 0;
        step(&mut world, 0.02);
        assert_eq!(world.cells.len(), 1);
        let cell = &world.cells[0];
        let radius = cell.values[6];
        // CellWorld.c() integrates drag before CellWorld.g() spends energy.
        let mass = 0.288;
        let force = radius * 60.0;
        let corrected_force = force * (1.0 - 0.02 * force * 0.5 / mass);
        let linear_factor = 1.0 - corrected_force * 0.02 / mass;
        let angular_factor = 1.0 - radius * 60.0 * 0.02 / (1.4 * mass);
        assert!((cell.values[2] - 0.2 * linear_factor).abs() < 1.0e-12);
        assert!((cell.values[3] - 0.1 * linear_factor).abs() < 1.0e-12);
        assert!((cell.values[4] - 0.5 * angular_factor).abs() < 1.0e-12);
    }

    #[test]
    fn apk_cell_friction_couples_linear_and_angular_drag() {
        let mut cell = legacy::import(bundled::PLATES[0].1)
            .unwrap()
            .cells
            .remove(0);
        cell.values[2] = 0.2;
        cell.values[3] = 0.0;
        cell.values[4] = 1.0;
        cell.values[6] = 0.02;
        cell.q = 0.0;
        let ratio: f64 = 0.2 / 0.02;
        let square = ratio * ratio;
        let linear_share = (0.1 * ratio + square) / (0.1 * ratio + square + 0.15);
        let expected_velocity = 0.2 - linear_share * 4.96 * 0.02;
        apply_cell_friction(&mut cell, 4.96, 0.0, 0.02);
        assert!((cell.values[2] - expected_velocity).abs() < 1.0e-12);
        assert_eq!(cell.values[3], 0.0);
        assert_eq!(cell.values[4], 0.0);
    }

    #[test]
    fn apk_static_friction_holds_a_resting_cell_until_force_wakes_it() {
        let mut world = legacy::import(bundled::PLATES[0].1).unwrap();
        world.cells.truncate(1);
        world.food.clear();
        world.environment.max_food = 0;
        world.environment.w = false;
        world.environment.i = 1.0;
        world.environment.j = 0.0;
        world.environment.g = 0.0;
        world.environment.m = 0.0;
        world.environment.l = 0.1;
        world.environment.h = 0.01;
        let cell = &mut world.cells[0];
        cell.mode = 0;
        cell.genes[0].cell_type = 0;
        cell.x = 0.0;
        cell.y = 0.0;
        cell.values[2..=5].fill(0.0);
        cell.values[6] = 0.02;
        cell.values[7] = 0.2;
        cell.values[8] = 0.0;
        cell.tuvp[3] = 1.0;
        cell.links.clear();
        cell.c = 0;
        cell.q = 0.0;
        cell.values[2] = 0.005;
        cell.resting = false;
        apply_cell_friction(cell, 1.0, 0.1, 0.02);
        assert_eq!(cell.values[2], 0.0);
        assert!(cell.resting);
        step(&mut world, 0.02);
        assert_eq!(world.cells[0].values[3], 0.0);
        assert!(world.cells[0].resting);

        world.environment.h = 0.1;
        step(&mut world, 0.02);
        assert!(world.cells[0].values[3] > 0.001);
        assert!(!world.cells[0].resting);
    }

    #[test]
    fn challenge_colors_use_stored_cell_color_not_visual_effects() {
        let mut world = legacy::import(bundled::PLATES[0].1).unwrap();
        world.cells.truncate(1);
        let cell = &mut world.cells[0];
        cell.e = 0;
        cell.color = [0.9, 0.1, 0.1];
        cell.tail[1] = 0.8;
        let snapshot = snapshot(&world, false, 1.0);
        assert!(snapshot.cells[0].color[1] > 0.65);
        assert_eq!(snapshot.cells[0].stored_color[1], 0.1);
        assert!(crate::levels::condition_met(
            &crate::levels::Condition {
                tag: "red-level-cells-min".into(),
                target: 1.0,
            },
            &snapshot,
        ));
        assert!(!crate::levels::condition_met(
            &crate::levels::Condition {
                tag: "yellow-level-cells-min".into(),
                target: 1.0,
            },
            &snapshot,
        ));
    }

    fn exchange_resources_for_test(world: &mut World, dt: f64) {
        let states = capture_resource_states(world);
        exchange_link_resources(world, dt, states);
    }

    #[test]
    fn linked_cells_exchange_mass_nutrient_and_toxin() {
        let mut world = legacy::import(bundled::PLATES[0].1).unwrap();
        world.environment.z = 0.0;
        world.cells.truncate(2);
        for (index, cell) in world.cells.iter_mut().enumerate() {
            cell.mode = 0;
            cell.genes[0].cell_type = 0;
            cell.genes[0].floats[0] = 0.2;
            cell.genes[0].floats[5] = if index == 0 { 1.0 } else { 2.0 };
            cell.genes[0].flags[3] = false;
            cell.values[7] = 0.2;
            cell.tuvp[3] = if index == 0 { 0.8 } else { 0.2 };
            cell.tail[0] = if index == 0 { 0.5 } else { 0.0 };
            cell.tail[1] = 0.0;
            cell.links.clear();
            cell.c = 0;
        }
        let (left, right) = world.cells.split_at_mut(1);
        add_link(&mut left[0], 1, 0.0, 0.0);
        add_link(&mut right[0], 0, 0.0, 0.0);
        add_link(&mut left[0], -1, 0.0, 0.0);

        exchange_resources_for_test(&mut world, 0.02);
        assert!((world.cells[0].values[7] - 0.198).abs() < 1.0e-8);
        assert!((world.cells[1].values[7] - 0.202).abs() < 1.0e-8);
        assert!((world.cells[0].tuvp[3] - 0.788).abs() < 1.0e-8);
        assert!((world.cells[1].tuvp[3] - 0.212).abs() < 1.0e-8);
        assert!((world.cells[0].tail[0] as f64 - 0.4695).abs() < 1.0e-6);
        assert!((world.cells[1].tail[0] as f64 - 0.03).abs() < 1.0e-6);
        assert!((world.cells[0].tail[1] as f64 - 0.0005).abs() < 1.0e-6);
    }

    #[test]
    fn linked_transfer_uses_mass_and_nutrient_before_growth() {
        let mut world = legacy::import(bundled::PLATES[0].1).unwrap();
        world.environment.z = 0.0;
        world.cells.truncate(2);
        for (index, cell) in world.cells.iter_mut().enumerate() {
            cell.mode = 0;
            cell.genes[0].cell_type = 0;
            cell.genes[0].floats[5] = if index == 0 { 1.0 } else { 2.0 };
            cell.genes[0].flags[3] = false;
            cell.values[7] = if index == 0 { 0.1 } else { 0.2 };
            cell.tuvp[3] = if index == 0 { 0.2 } else { 0.8 };
            cell.links.clear();
            cell.c = 0;
        }
        let (left, right) = world.cells.split_at_mut(1);
        add_link(&mut left[0], 1, 0.0, 0.0);
        add_link(&mut right[0], 0, 0.0, 0.0);
        let original = capture_resource_states(&world);
        world.cells[0].values[7] = 0.3;
        world.cells[0].tuvp[3] = 0.5;
        exchange_link_resources(&mut world, 0.02, original);
        assert!((world.cells[0].values[7] - 0.3).abs() < 1.0e-8);
        assert!((world.cells[1].values[7] - 0.2).abs() < 1.0e-8);
        assert!((world.cells[0].tuvp[3] - 0.212).abs() < 1.0e-8);
        assert!((world.cells[1].tuvp[3] - 0.788).abs() < 1.0e-8);
    }

    #[test]
    fn starving_linked_cell_increases_its_transport_rate() {
        let mut world = legacy::import(bundled::PLATES[0].1).unwrap();
        world.cells.truncate(2);
        for (index, cell) in world.cells.iter_mut().enumerate() {
            cell.mode = 0;
            cell.genes[0].cell_type = 0;
            cell.genes[0].floats[0] = 0.2;
            cell.genes[0].floats[5] = if index == 0 { 0.4 } else { 1.0 };
            cell.genes[0].flags[3] = false;
            cell.values[7] = if index == 0 { 0.08 } else { 0.2 };
            cell.tuvp[3] = 1.0;
            cell.links.clear();
            cell.c = 0;
        }
        let (left, right) = world.cells.split_at_mut(1);
        add_link(&mut left[0], 1, 0.0, 0.0);
        add_link(&mut right[0], 0, 0.0, 0.0);
        let mut starving = world.clone();
        starving.cells[0].genes[0].flags[3] = true;

        exchange_resources_for_test(&mut world, 0.02);
        exchange_resources_for_test(&mut starving, 0.02);
        assert!((world.cells[0].values[7] - 0.08).abs() < 1.0e-7);
        assert!((starving.cells[0].values[7] - 0.082).abs() < 1.0e-7);
        assert!((starving.cells[1].values[7] - 0.198).abs() < 1.0e-7);
    }

    #[test]
    fn linked_resource_exchange_is_identical_with_one_or_two_workers() {
        let mut world = legacy::import(bundled::PLATES[0].1).unwrap();
        let template = world.cells[0].clone();
        world.cells.clear();
        for index in 0..64 {
            let mut cell = template.clone();
            cell.mode = 0;
            cell.genes[0].cell_type = 0;
            cell.genes[0].floats[5] = 0.5 + (index % 3) as f32;
            cell.values[7] = 0.2;
            cell.tuvp[3] = 0.2 + index as f64 / 100.0;
            cell.tail[0] = index as f32 / 80.0;
            cell.links.clear();
            cell.c = 0;
            add_link(&mut cell, (index ^ 1) as i32, 0.0, 0.0);
            world.cells.push(cell);
        }
        let mut one = world.clone();
        let mut two = world;
        let pool_one = rayon::ThreadPoolBuilder::new()
            .num_threads(1)
            .build()
            .unwrap();
        let pool_two = rayon::ThreadPoolBuilder::new()
            .num_threads(2)
            .build()
            .unwrap();
        for _ in 0..20 {
            pool_one.install(|| exchange_resources_for_test(&mut one, 0.02));
            pool_two.install(|| exchange_resources_for_test(&mut two, 0.02));
        }
        assert_eq!(
            serde_json::to_vec(&one).unwrap(),
            serde_json::to_vec(&two).unwrap()
        );
    }

    #[test]
    fn linked_cells_exchange_friction_strain_and_muscles_drive_it() {
        let mut world = legacy::import(bundled::PLATES[0].1).unwrap();
        let mut lower = world.cells[0].clone();
        let mut higher = lower.clone();
        lower.links.clear();
        higher.links.clear();
        lower.c = 0;
        higher.c = 0;
        lower.mode = 0;
        higher.mode = 0;
        lower.genes[0].cell_type = 0;
        higher.genes[0].cell_type = 0;
        lower.q = 0.0;
        higher.q = 0.2;
        add_link(&mut lower, 1, 0.0, 0.0);
        add_link(&mut higher, 0, 0.0, 0.0);
        world.cells = vec![lower, higher];
        solve_links(&mut world, 0.02);
        assert!((world.cells[0].q - 0.04).abs() < 1.0e-8);
        assert!((world.cells[1].q - 0.148).abs() < 1.0e-8);

        world.cells[0].q = 0.0;
        world.cells[1].q = 0.0;
        world.cells[1].genes[0].cell_type = 12;
        world.cells[1].tail[4] = 0.5;
        solve_links(&mut world, 0.02);
        assert!((world.cells[0].q - 0.1).abs() < 1.0e-8);
        assert!((world.cells[1].q + 0.1).abs() < 1.0e-8);
    }

    #[test]
    fn grabbed_cells_keep_position_and_primary_angle_while_their_state_advances() {
        let mut held = legacy::import(bundled::PLATES[0].1).unwrap();
        held.cells.truncate(1);
        held.food.clear();
        held.environment.max_cells = 1;
        held.environment.c = 0.0;
        let cell = &mut held.cells[0];
        cell.x = 0.0;
        cell.y = 0.0;
        cell.values[0] = 0.0;
        cell.values[1] = 0.0;
        cell.values[2] = 0.2;
        cell.values[3] = 0.0;
        cell.values[4] = 1.0;
        cell.values[5] = 0.5;
        cell.values[7] = 0.3;
        cell.values[8] = 0.0;
        cell.links.clear();
        cell.c = 0;
        cell.f = 100;
        let mut free = held.clone();
        free.cells[0].f = 0;

        step(&mut held, 0.02);
        step(&mut free, 0.02);
        assert_eq!(held.cells.len(), 1);
        assert_eq!(free.cells.len(), 1);
        assert_eq!(held.cells[0].x, 0.0);
        assert_eq!(held.cells[0].y, 0.0);
        assert_eq!(held.cells[0].values[0], 0.0);
        assert!(held.cells[0].values[1] > 0.0);
        assert!(free.cells[0].x > 0.0);
        assert!(free.cells[0].values[0] > 0.0);
    }

    #[test]
    fn engulfed_cell_is_removed_on_the_next_step_in_either_contact_order() {
        let mut world = legacy::import(bundled::PLATES[0].1).unwrap();
        let mut large = world.cells[0].clone();
        large.mode = 0;
        large.genes[0].cell_type = 0;
        large.x = 0.0;
        large.y = 0.0;
        large.values[2..=5].fill(0.0);
        large.values[6] = 0.03;
        large.values[7] = 0.216;
        large.values[8] = 0.0;
        large.tuvp[3] = 1.0;
        large.tail[1] = 0.0;
        large.x_flag = false;
        large.links.clear();
        large.c = 0;
        large.f = 0;
        let mut small = large.clone();
        small.x = 0.015;
        small.values[6] = MIN_CELL_RADIUS;
        world.cells = vec![large, small];
        world.food.clear();
        world.environment.max_food = 0;
        world.environment.w = false;
        let mut reversed = world.clone();
        reversed.cells.reverse();

        step(&mut world, 0.02);
        step(&mut reversed, 0.02);
        for result in [&mut world, &mut reversed] {
            assert_eq!(result.cells.len(), 2);
            assert!(result.cells.iter().any(|cell| cell.x_flag));
            step(result, 0.02);
            assert_eq!(result.cells.len(), 1);
            assert_eq!(result.cells[0].values[6], 0.03);
        }
    }

    #[test]
    fn engulfment_precedes_devorocyte_feeding_across_the_plate() {
        let mut world = legacy::import(bundled::PLATES[0].1).unwrap();
        let mut child = world.cells[0].clone();
        child.mode = 0;
        child.genes[0].cell_type = 0;
        child.x = -0.025;
        child.y = 0.0;
        child.values[6] = 0.02;
        child.values[7] = 0.2;
        child.x_flag = false;
        child.links.clear();
        child.c = 0;
        let mut devorocyte = child.clone();
        devorocyte.genes[0].cell_type = 3;
        devorocyte.x = 0.0;
        let mut larger = child.clone();
        larger.x = 0.01;
        larger.values[6] = 0.03;
        world.cells = vec![child, devorocyte, larger];

        solve_contacts(&mut world, 0.02, false, &mut rand::rng());

        assert!(world.cells[1].x_flag);
        assert!(!world.cells[0].x_flag);
        assert_eq!(world.cells[0].values[7], 0.2);
    }

    #[test]
    fn newly_fertilized_keratinocyte_is_protected_in_the_same_contact_step() {
        let mut world = legacy::import(bundled::PLATES[0].1).unwrap();
        let mut survivor = world.cells[0].clone();
        survivor.mode = 0;
        survivor.genes[0].cell_type = 16;
        survivor.genes[0].output_modes[0] = 1;
        survivor.genes[1].cell_type = 5;
        survivor.x = 0.0;
        survivor.y = 0.0;
        survivor.values[6] = 0.03;
        survivor.values[7] = 0.15;
        survivor.values[8] = 1.0;
        survivor.x_flag = false;
        survivor.links.clear();
        survivor.c = 0;
        let mut donor = survivor.clone();
        donor.x = 0.01;
        donor.values[6] = 0.015;
        donor.values[7] = 0.1;
        let mut devorocyte = survivor.clone();
        devorocyte.genes[0].cell_type = 3;
        devorocyte.x = -0.04;
        devorocyte.values[6] = 0.02;
        devorocyte.values[7] = 0.2;
        world.cells = vec![survivor, donor, devorocyte];
        let mut rng = <rand::rngs::StdRng as rand::SeedableRng>::seed_from_u64(7);

        solve_contacts(&mut world, 0.02, false, &mut rng);

        assert_eq!(world.cells[0].cell_type(), 5);
        assert!(world.cells[1].x_flag);
        assert_eq!(world.cells[2].values[7], 0.2);
        assert_eq!(world.cells[0].values[7], 0.25);
    }

    #[test]
    fn contacts_push_neighbors_away_without_moving_a_grabbed_cell() {
        let mut world = legacy::import(bundled::PLATES[0].1).unwrap();
        world.cells.truncate(2);
        world.food.clear();
        world.environment.max_cells = 2;
        world.environment.c = 0.0;
        for cell in &mut world.cells {
            cell.genes[cell.mode as usize].cell_type = 0;
            cell.y = 0.0;
            cell.values[2] = 0.0;
            cell.values[3] = 0.0;
            cell.values[6] = 0.02;
            cell.values[7] = 0.3;
            cell.values[8] = 0.0;
            cell.links.clear();
            cell.c = 0;
            cell.f = 0;
        }
        world.cells[0].x = 0.0;
        world.cells[0].f = 100;
        world.cells[1].x = 0.02;
        step(&mut world, 0.02);
        assert_eq!(world.cells.len(), 2);
        assert_eq!(world.cells[0].x, 0.0);
        assert_eq!(world.cells[0].y, 0.0);
        assert!(world.cells[1].values[2] > 0.0);
        step(&mut world, 0.02);
        assert!(world.cells[1].x > 0.02);
    }

    #[test]
    fn photocytes_buoyocytes_and_gametes_track_secondary_rotation() {
        let mut base = legacy::import(bundled::PLATES[0].1).unwrap();
        base.cells.truncate(1);
        base.food.clear();
        base.environment.a = 0.0;
        base.environment.w = false;
        base.environment.g = 0.0;
        base.environment.max_cells = 1;
        for kind in [0, 2, 6, 16] {
            let mut world = base.clone();
            let cell = &mut world.cells[0];
            cell.x = 0.0;
            cell.y = 0.0;
            cell.mode = 0;
            cell.genes[0].cell_type = kind;
            cell.values[1] = 0.0;
            cell.values[4] = 2.0;
            cell.values[5] = 0.0;
            cell.values[6] = 0.02;
            cell.values[7] = 0.2;
            cell.values[8] = 0.0;
            step(&mut world, 0.02);
            assert!((world.cells[0].values[5] - if kind == 0 { 0.0 } else { 0.08 }).abs() < 1e-10);
            assert_eq!(world.cells[0].values[1], 0.0);
        }
    }

    #[test]
    fn glueocyte_spikes_follow_motion_and_relax_when_still() {
        let mut world = legacy::import(bundled::PLATES[0].1).unwrap();
        world.cells.truncate(1);
        world.food.clear();
        world.environment.a = 0.0;
        world.environment.w = false;
        world.environment.g = 0.0;
        world.environment.max_cells = 1;
        let cell = &mut world.cells[0];
        cell.mode = 0;
        cell.genes[0].cell_type = 7;
        cell.x = 0.0;
        cell.y = 0.0;
        cell.x_flag = false;
        cell.values[2] = 0.3;
        cell.values[3] = -0.2;
        cell.values[4] = 0.5;
        cell.values[6] = 0.02;
        cell.values[7] = 0.2;
        cell.values[8] = 0.0;
        cell.tuvp[0..3].fill(0.0);
        cell.tail[1] = 0.0;
        step(&mut world, 0.02);
        assert_eq!(world.cells.len(), 1);
        assert!(world.cells[0].tuvp[0] < 0.0);
        assert!(world.cells[0].tuvp[1] < 0.0);
        assert!(world.cells[0].tuvp[2] > 0.0);
        let motion = world.cells[0].tuvp[1].hypot(world.cells[0].tuvp[2]);
        world.cells[0].values[2..=4].fill(0.0);
        step(&mut world, 0.02);
        assert!(world.cells[0].tuvp[1].hypot(world.cells[0].tuvp[2]) < motion);
    }

    #[test]
    fn clustered_food_respects_plate_and_capacity() {
        let mut world = legacy::import(bundled::PLATES[0].1).unwrap();
        world.food.clear();
        world.environment.a = 500.0;
        world.environment.b = 0.15;
        world.environment.p = 3.0;
        world.environment.q = 0.02;
        world.environment.max_food = 100;
        spawn_food(&mut world, 0.02);
        assert!(!world.food.is_empty());
        assert!(world.food.len() <= 100);
        assert!(
            world
                .food
                .iter()
                .all(|f| (f.x as f64).hypot(f.y as f64) < world.environment.radius)
        );
    }

    #[test]
    fn automatic_seeding_uses_enabled_cell_types() {
        let mut world = legacy::import(bundled::PLATES[0].1).unwrap();
        world.cells.clear();
        world.environment.max_cells = 100;
        world.environment.w = true;
        world.environment.enabled_types = vec![false; 18];
        world.environment.enabled_types[0] = true;
        spawn_random_cells(&mut world);
        assert!((10..=11).contains(&world.cells.len()));
        assert!(world.cells.iter().all(|cell| cell.e == 2
            && cell.genes.len() == 40
            && cell.genes.iter().all(|gene| gene.cell_type == 0)));
    }

    #[test]
    fn seeded_lipocytes_start_with_the_apk_reserve_and_split_ratio_range() {
        let mut rng = <rand::rngs::StdRng as rand::SeedableRng>::seed_from_u64(7);
        let cell = random_cell(0.0, 0.0, &[4], &mut rng);
        assert_eq!(cell.e, 2);
        assert_eq!(cell.cell_type(), 4);
        assert!(cell.tail[5] > 0.0 && cell.tail[5] < 1.8);
        assert!(
            cell.genes
                .iter()
                .all(|gene| (0.3..=0.7).contains(&gene.floats[1]))
        );
        assert!(cell.genes.iter().any(|gene| gene.floats[1] < 0.4));
        assert!(cell.genes.iter().any(|gene| gene.floats[1] > 0.6));
    }

    #[test]
    fn stem_cell_switches_to_gene_selected_mode() {
        let mut world = legacy::import(bundled::PLATES[0].1).unwrap();
        world.cells.truncate(1);
        world.food.clear();
        world.environment.w = false;
        world.environment.a = 0.0;
        let cell = &mut world.cells[0];
        cell.mode = 0;
        cell.genes[0].cell_type = 15;
        cell.genes[0].output_modes[0] = 1;
        cell.genes[0].inputs[5] = GeneInput {
            source: 0,
            mode: 0,
            a: 0.0,
            b: 1.0,
            c: 0.0,
        };
        cell.genes[1].cell_type = 2;
        cell.values[8] = 0.6;
        cell.values[7] = 0.2;
        cell.x = 0.0;
        cell.y = 0.0;
        cell.x_flag = false;
        step(&mut world, 0.02);
        assert_eq!(world.cells[0].mode, 1);
        assert!(world.cells[0].values[8] < 0.1);
    }

    #[test]
    fn buoyancy_and_lipocyte_reserve_affect_motion_and_mass() {
        let mut world = legacy::import(bundled::PLATES[0].1).unwrap();
        world.cells.truncate(1);
        world.food.clear();
        world.environment.w = false;
        world.environment.a = 0.0;
        world.environment.i = 2.0;
        world.environment.h = 1.0;
        world.environment.j = 0.0;
        let cell = &mut world.cells[0];
        cell.mode = 0;
        cell.genes[0].cell_type = 6;
        cell.genes[0].inputs[1] = GeneInput {
            source: 0,
            mode: 0,
            a: 0.0,
            b: -1.0,
            c: 0.0,
        };
        cell.x = 0.0;
        cell.y = 0.0;
        cell.x_flag = false;
        cell.values[2] = 0.0;
        cell.values[3] = 0.0;
        cell.values[7] = 0.2;
        cell.values[8] = 0.0;
        cell.links.clear();
        cell.c = 0;
        let mut without_buoyancy = world.clone();
        without_buoyancy.cells[0].genes[0].inputs[1].b = 0.0;
        step(&mut world, 0.02);
        step(&mut without_buoyancy, 0.02);
        assert_eq!(world.cells.len(), 1);
        assert_eq!(without_buoyancy.cells.len(), 1);
        assert!(world.cells[0].values[3] > without_buoyancy.cells[0].values[3] + 0.01);
        world.environment.i = 0.0;
        world.cells[0].mode = 0;
        world.cells[0].values[8] = 0.0;
        world.cells[0].genes[0].cell_type = 4;
        world.cells[0].values[7] = 0.35;
        world.cells[0].tail[5] = 0.1;
        step(&mut world, 0.02);
        assert!(world.cells[0].values[7] > 0.359);
        assert!(world.cells[0].tail[5] < 0.1);
    }

    #[test]
    fn nutrient_depletion_flags_a_cell_for_next_step_cleanup() {
        let mut world = legacy::import(bundled::PLATES[0].1).unwrap();
        world.cells.truncate(2);
        world.food.clear();
        world.environment.max_food = 0;
        world.environment.w = false;
        world.environment.z = 0.0;
        for (index, cell) in world.cells.iter_mut().enumerate() {
            cell.x = if index == 0 { -0.3 } else { 0.3 };
            cell.y = 0.0;
            cell.values[2] = 0.0;
            cell.values[3] = 0.0;
            cell.values[6] = (0.216_f64 / 400.0).sqrt();
            cell.values[7] = 0.216;
            cell.values[8] = 1.0;
            cell.tail[1] = 0.0;
            cell.x_flag = false;
            cell.links.clear();
            cell.c = 0;
            cell.tuvp[3] = if index == 0 { 0.05 } else { 1.3 };
        }
        step(&mut world, 0.02);
        assert_eq!(world.cells.len(), 2);
        assert!(world.cells[0].x_flag);
        assert!(!world.cells[1].x_flag);
        assert_eq!(world.cells[1].tuvp[3], 1.0);
        step(&mut world, 0.02);
        assert_eq!(world.cells.len(), 1);
        assert_eq!(world.cells[0].x, 0.3);
    }

    #[test]
    fn ambient_nutrient_and_nitrocyte_output_do_not_hide_starvation() {
        let mut world = legacy::import(bundled::PLATES[0].1).unwrap();
        world.cells.truncate(2);
        world.food.clear();
        world.environment.max_food = 0;
        world.environment.w = false;
        world.environment.z = 0.3;
        for (index, cell) in world.cells.iter_mut().enumerate() {
            cell.mode = 0;
            cell.genes[0].cell_type = if index == 0 { 0 } else { 9 };
            cell.x = if index == 0 { -0.3 } else { 0.3 };
            cell.y = 0.0;
            cell.values[7] = 0.2;
            cell.values[8] = 0.0;
            cell.tuvp[3] = 0.099;
            cell.x_flag = false;
            cell.links.clear();
            cell.c = 0;
        }
        step(&mut world, 0.02);
        assert!(world.cells.iter().all(|cell| cell.x_flag));
        assert!(world.cells[0].tuvp[3] > 0.1);
        assert_eq!(world.cells[1].tuvp[3], 1.0);
    }

    #[test]
    fn contact_effects_transfer_energy_toxin_and_links() {
        let mut world = legacy::import(bundled::PLATES[0].1).unwrap();
        world.cells.truncate(2);
        let (left, right) = world.cells.split_at_mut(1);
        let first = &mut left[0];
        let second = &mut right[0];
        first.mode = 0;
        second.mode = 0;
        first.values[7] = 0.2;
        second.values[7] = 0.2;
        first.values[8] = 1.0;
        second.values[8] = 1.0;
        first.tuvp[3] = 1.0;
        second.tuvp[3] = 0.6;
        first.tail[0] = 0.1;
        second.tail[0] = 0.4;
        first.links.clear();
        second.links.clear();
        first.c = 0;
        second.c = 0;
        first.x = 0.0;
        first.y = 0.0;
        second.x = 0.02;
        second.y = 0.0;
        first.genes[0].cell_type = 3;
        second.genes[0].cell_type = 0;
        let mut rng = rand::rng();
        pair_effects(first, second, 0, 1, 0.02, false, false, false, &mut rng);
        assert!((first.values[7] - 0.2144).abs() < 1.0e-8);
        assert!((second.values[7] - 0.1856).abs() < 1.0e-8);
        assert!(first.tuvp[3] > 1.0);
        assert!(second.tuvp[3] < 0.6);
        assert!((first.tuvp[3] + second.tuvp[3] - 1.6).abs() < 1.0e-8);
        assert!(first.tail[0] > 0.1);
        assert!((first.tail[0] + second.tail[0] - 0.5).abs() < 1.0e-5);
        first.genes[0].cell_type = 14;
        first.genes[0].output_modes[4] = 2;
        pair_effects(first, second, 0, 1, 0.02, false, false, false, &mut rng);
        assert!(second.tail[1] > 0.0019);
        first.genes[0].output_modes[4] = 1;
        let protected_toxin = second.tail[0];
        pair_effects(first, second, 0, 1, 0.02, false, true, false, &mut rng);
        assert_eq!(second.tail[0], protected_toxin);
        pair_effects(first, second, 0, 1, 0.02, false, false, false, &mut rng);
        assert!(second.tail[0] > protected_toxin);
        first.genes[0].cell_type = 7;
        pair_effects(first, second, 0, 1, 0.02, false, false, false, &mut rng);
        assert_eq!(first.c, 1);
        assert_eq!(second.c, 1);
        assert_eq!(first.links[0].target, 1);
        assert_eq!(second.links[0].target, 0);
    }

    #[test]
    fn secrocyte_cuts_reciprocal_links_and_plate_anchors() {
        let mut world = legacy::import(bundled::PLATES[0].1).unwrap();
        world.cells.truncate(3);
        for cell in &mut world.cells {
            cell.mode = 0;
            cell.genes[0].cell_type = 0;
            cell.x_flag = false;
            cell.y = 0.0;
            cell.values[2] = 0.0;
            cell.values[3] = 0.0;
            cell.values[6] = 0.02;
            cell.values[7] = 0.2;
            cell.values[8] = 1.0;
            cell.tuvp[3] = 1.0;
            cell.links.clear();
            cell.c = 0;
        }
        world.cells[0].genes[0].cell_type = 14;
        world.cells[0].genes[0].output_modes[4] = 3;
        world.cells[0].x = 0.0;
        world.cells[1].x = 0.02;
        world.cells[2].x = -0.02;
        add_link(&mut world.cells[1], 2, 0.0, 0.0);
        add_link(&mut world.cells[2], 1, 0.0, 0.0);
        solve_contacts(&mut world, 0.02, false, &mut rand::rng());
        assert_eq!(world.cells[1].c, 0);
        assert_eq!(world.cells[2].c, 0);

        world.cells.truncate(2);
        world.cells[0].x = world.environment.radius - 0.015;
        world.cells[1].x = world.environment.radius - 0.03;
        add_link(&mut world.cells[1], -1, 0.0, 0.0);
        solve_contacts(&mut world, 0.02, false, &mut rand::rng());
        assert_eq!(world.cells[1].c, 0);
    }

    #[test]
    fn virocyte_copies_genes_and_marks_infected_origin() {
        let mut world = legacy::import(bundled::PLATES[0].1).unwrap();
        world.cells.truncate(2);
        let (left, right) = world.cells.split_at_mut(1);
        let source = &mut left[0];
        let victim = &mut right[0];
        source.mode = 0;
        source.genes[0].cell_type = 8;
        source.genes[0].output_modes[0] = 3;
        source.genes[3].color = [1.0, 0.0, 0.0];
        victim.genes[3].color = [0.0, 0.0, 1.0];
        victim.e = 0;
        infect(source, victim);
        assert_eq!(victim.e, 3);
        assert_eq!(victim.genes[3].color, [1.0, 0.0, 0.0]);
    }

    #[test]
    fn genome_preview_disables_flagellum_force_gravity_and_food_updates() {
        let mut preview = legacy::import(bundled::PLATES[0].1).unwrap();
        preview.cells.truncate(1);
        preview.food.clear();
        preview.environment.a = 0.0;
        preview.environment.w = false;
        preview.environment.g = 0.0;
        preview.environment.m = 0.0;
        preview.environment.l = 0.0;
        preview.environment.i = 1.0;
        preview.environment.h = 1.0;
        preview.environment.max_cells = 1;
        preview.food.push(Food {
            x: 0.5,
            y: 0.0,
            energy: 1.0,
            vx: 0.0,
            vy: 0.0,
            state: 0.0,
        });
        let cell = &mut preview.cells[0];
        cell.mode = 0;
        cell.genes[0].cell_type = 1;
        cell.genes[0].inputs[0].mode = 0;
        cell.genes[0].inputs[0].b = 1.0;
        cell.x = 0.0;
        cell.y = 0.0;
        cell.values[0] = 0.0;
        cell.values[2..=5].fill(0.0);
        cell.values[6] = 0.02;
        cell.values[7] = 0.2;
        cell.values[8] = 0.0;
        cell.x_flag = false;
        cell.links.clear();
        cell.c = 0;
        let mut plate = preview.clone();

        let mut rng = <rand::rngs::StdRng as rand::SeedableRng>::seed_from_u64(7);
        step_preview(&mut preview, 0.02, &mut rng);
        step(&mut plate, 0.02);
        assert_eq!(preview.cells[0].values[2], 0.0);
        assert_eq!(preview.cells[0].values[3], 0.0);
        assert!(preview.cells[0].visual_phase > 0.0);
        assert_eq!(preview.food[0].energy, 1.0);
        assert!(plate.cells[0].values[2] > 0.0);
        assert!(plate.cells[0].values[3] > 0.0);
        assert!(plate.food[0].energy < 1.0);
    }

    #[test]
    fn genome_preview_disables_virocyte_gene_copying() {
        let world = legacy::import(bundled::PLATES[0].1).unwrap();
        let mut source = world.cells[0].clone();
        let mut victim = world.cells[1].clone();
        source.mode = 0;
        source.genes[0].cell_type = 8;
        source.genes[0].output_modes[0] = 3;
        source.genes[3].color = [1.0, 0.0, 0.0];
        victim.mode = 0;
        victim.genes[0].cell_type = 0;
        victim.genes[3].color = [0.0, 0.0, 1.0];
        victim.e = 0;
        let mut plate_source = source.clone();
        let mut plate_victim = victim.clone();

        pair_effects(
            &mut source,
            &mut victim,
            0,
            1,
            1.0,
            false,
            false,
            true,
            &mut rand::rng(),
        );
        pair_effects(
            &mut plate_source,
            &mut plate_victim,
            0,
            1,
            1.0,
            false,
            false,
            false,
            &mut rand::rng(),
        );
        assert_eq!(victim.e, 0);
        assert_eq!(victim.genes[3].color, [0.0, 0.0, 1.0]);
        assert_eq!(plate_victim.e, 3);
        assert_eq!(plate_victim.genes[3].color, [1.0, 0.0, 0.0]);
    }

    #[test]
    fn compatible_gametes_fuse_into_a_hybrid() {
        let mut world = legacy::import(bundled::PLATES[0].1).unwrap();
        world.cells.truncate(2);
        world.food.clear();
        world.environment.w = false;
        world.environment.a = 0.0;
        for (index, cell) in world.cells.iter_mut().enumerate() {
            cell.mode = 0;
            cell.genes[0].cell_type = 16;
            cell.genes[0].output_modes[1] = 0;
            cell.values[6] = 0.02;
            cell.values[7] = 0.2;
            cell.values[8] = 1.0;
            cell.values[2] = 0.0;
            cell.values[3] = 0.0;
            cell.x = index as f64 * 0.001;
            cell.y = 0.0;
            cell.x_flag = false;
            cell.e = index as i32;
            cell.links.clear();
            cell.c = 0;
        }
        step(&mut world, 0.02);
        assert_eq!(world.cells.len(), 2);
        let survivor = world.cells.iter().find(|cell| !cell.x_flag).unwrap();
        assert_eq!(survivor.e, 6);
        assert!(survivor.values[7] > 0.3);
        assert_eq!(world.cells.iter().filter(|cell| cell.x_flag).count(), 1);
        step(&mut world, 0.02);
        assert_eq!(world.cells.len(), 1);
        assert_eq!(world.cells[0].e, 6);
    }

    #[test]
    fn compatible_gametes_attract_through_contact_force() {
        let mut world = legacy::import(bundled::PLATES[0].1).unwrap();
        world.cells.truncate(2);
        world.food.clear();
        world.environment.w = false;
        world.environment.a = 0.0;
        world.environment.g = 0.0;
        world.environment.m = 0.0;
        world.environment.l = 0.0;
        for (index, cell) in world.cells.iter_mut().enumerate() {
            cell.mode = 0;
            cell.genes[0].cell_type = 16;
            cell.genes[0].output_modes[1] = 0;
            cell.values[6] = 0.02;
            cell.values[7] = 0.2;
            cell.values[8] = 1.0;
            cell.values[2..=5].fill(0.0);
            cell.x = index as f64 * 0.02;
            cell.y = 0.0;
            cell.x_flag = false;
            cell.links.clear();
            cell.c = 0;
        }
        step(&mut world, 0.02);
        assert_eq!(world.cells.len(), 2);
        assert_eq!(world.cells[0].x, 0.0);
        assert_eq!(world.cells[1].x, 0.02);
        assert!(world.cells[0].values[2] > 0.0);
        assert!(world.cells[1].values[2] < 0.0);
    }

    #[test]
    fn unequal_gametes_fuse_before_engulfment_removes_the_smaller_one() {
        let mut world = legacy::import(bundled::PLATES[0].1).unwrap();
        world.cells.truncate(2);
        world.food.clear();
        world.environment.w = false;
        world.environment.a = 0.0;
        for (index, cell) in world.cells.iter_mut().enumerate() {
            cell.mode = 0;
            cell.genes[0].cell_type = 16;
            cell.values[6] = if index == 0 { 0.03 } else { MIN_CELL_RADIUS };
            cell.values[7] = 0.2;
            cell.values[8] = 1.0;
            cell.values[2..=5].fill(0.0);
            cell.x = index as f64 * 0.01;
            cell.y = 0.0;
            cell.x_flag = false;
            cell.e = index as i32;
            cell.links.clear();
            cell.c = 0;
        }
        let mut reversed = world.clone();
        reversed.cells.reverse();
        for result in [&mut world, &mut reversed] {
            step(result, 0.02);
            assert_eq!(result.cells.len(), 2);
            let survivor = result.cells.iter().find(|cell| !cell.x_flag).unwrap();
            assert_eq!(survivor.e, 6, "fusion combines the two origins");
            assert!(survivor.values[7] > 0.3, "fusion transfers donor mass");
            assert_eq!(result.cells.iter().filter(|cell| cell.x_flag).count(), 1);
            step(result, 0.02);
            assert_eq!(result.cells.len(), 1);
            assert_eq!(result.cells[0].e, 6);
        }
    }

    #[test]
    fn near_equal_gametes_can_fuse_in_either_direction() {
        let world = legacy::import(bundled::PLATES[0].1).unwrap();
        let mut first = world.cells[0].clone();
        let mut second = world.cells[1].clone();
        first.mode = 0;
        second.mode = 0;
        first.genes[0].cell_type = 16;
        second.genes[0].cell_type = 16;
        first.values[6] = 0.020_01;
        second.values[6] = 0.02;
        first.values[7] = 0.1;
        second.values[7] = 0.1;
        first.x = 0.0;
        first.y = 0.0;
        second.x = 0.001;
        second.y = 0.0;
        let mut first_wins = false;
        let mut second_wins = false;
        for seed in 0..32 {
            let mut a = first.clone();
            let mut b = second.clone();
            let mut rng = <rand::rngs::StdRng as rand::SeedableRng>::seed_from_u64(seed);
            assert!(gamete_contact(&mut a, &mut b, &mut rng));
            first_wins |= !a.x_flag;
            second_wins |= !b.x_flag;
        }
        assert!(
            first_wins && second_wins,
            "the APK randomizes near-equal fusions"
        );
    }
}
