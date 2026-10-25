mod apk_cells;
mod apk_dead;
mod apk_food;
mod apk_links;
mod apk_plate;
mod audio;
mod bundled;
mod file_dialog;
mod legacy;
mod levels;
mod model;
mod mutation;
mod native;
mod oscillator;
mod preview;
mod progress;
mod sense_kernel;
mod sensing;
mod sim;
mod ui;
mod visuals;

fn main() {
    let arguments: Vec<String> = std::env::args().collect();
    if arguments.get(1).is_some_and(|value| value == "--benchmark") {
        if let Err(error) = benchmark(&arguments) {
            eprintln!("Benchmark failed: {error}");
            std::process::exit(1);
        }
        return;
    }
    if arguments
        .get(1)
        .is_some_and(|value| value == "--benchmark-snapshot")
    {
        if let Err(error) = benchmark_snapshot(&arguments) {
            eprintln!("Snapshot benchmark failed: {error}");
            std::process::exit(1);
        }
        return;
    }
    if arguments.get(1).is_some_and(|value| value == "--edit-save") {
        if let Err(error) = edit_save(&arguments) {
            eprintln!("Edit failed: {error}");
            std::process::exit(1);
        }
        return;
    }
    if arguments
        .get(1)
        .is_some_and(|value| value == "--export-genome")
    {
        if let Err(error) = export_genome(&arguments) {
            eprintln!("Genome export failed: {error}");
            std::process::exit(1);
        }
        return;
    }
    if arguments.len() == 3 && arguments[1] == "--inspect" {
        match native::load(&arguments[2]) {
            Ok(world) => println!(
                "Cell Lab save v{:?}: {} cells, {} food, radius {}, limits {}/{}",
                world.source_version,
                world.cells.len(),
                world.food.len(),
                world.environment.radius,
                world.environment.max_cells,
                world.environment.max_food,
            ),
            Err(error) => {
                eprintln!("Import failed: {error}");
                std::process::exit(1);
            }
        }
        return;
    }
    let capture_path = (arguments.len() >= 3 && arguments[1] == "--capture-ui")
        .then(|| std::path::PathBuf::from(&arguments[2]));
    let capture_plate = capture_path
        .as_ref()
        .and_then(|_| arguments.get(3).cloned());
    let capture_zoom = capture_path
        .as_ref()
        .and_then(|_| arguments.get(4)?.parse::<f32>().ok());
    let capture_pan_y = capture_path
        .as_ref()
        .and_then(|_| arguments.get(5)?.parse::<f32>().ok());
    let capture_preview_time = capture_path
        .as_ref()
        .and_then(|_| arguments.get(6)?.parse::<f64>().ok());
    let mut viewport = eframe::egui::ViewportBuilder::default()
        .with_inner_size([1280.0, 800.0])
        .with_min_inner_size([1024.0, 600.0]);
    if capture_path.is_some() {
        viewport = viewport
            .with_position([-3000.0, -3000.0])
            .with_active(false);
    }
    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    eframe::run_native(
        "Cell Lab 2",
        options,
        Box::new(move |cc| {
            Ok(Box::new(ui::App::new(
                cc,
                capture_path.clone(),
                capture_plate.clone(),
                capture_zoom,
                capture_pan_y,
                capture_preview_time,
            )))
        }),
    )
    .expect("desktop application failed");
}

fn benchmark(arguments: &[String]) -> Result<(), String> {
    if !(4..=5).contains(&arguments.len()) {
        return Err("Usage: celllab2 --benchmark INPUT STEPS [CELL_COUNT]".into());
    }
    let mut world = native::load(&arguments[2])?;
    let steps: usize = arguments[3].parse().map_err(|_| "Invalid step count")?;
    if let Some(requested) = arguments.get(4) {
        let requested: usize = requested.parse().map_err(|_| "Invalid cell count")?;
        if world.cells.is_empty() {
            return Err("The input has no organism template".into());
        }
        let source = world.cells.clone();
        while world.cells.len() < requested {
            let index = world.cells.len();
            let mut cell = source[index % source.len()].clone();
            cell.runtime_id = model::fresh_cell_id();
            let distance =
                world.environment.radius * 0.8 * ((index as f64 / requested as f64).sqrt());
            let angle = index as f64 * 2.399963229728653;
            cell.x = distance * angle.cos();
            cell.y = distance * angle.sin();
            cell.links.clear();
            cell.c = 0;
            world.cells.push(cell);
        }
        world.environment.max_cells = world.environment.max_cells.max(requested);
    }
    let start = std::time::Instant::now();
    let mut timings = sim::StepTimings::default();
    for _ in 0..steps {
        timings += sim::step(&mut world, 0.02);
    }
    let elapsed = start.elapsed().as_secs_f64();
    println!(
        "{steps} steps in {elapsed:.3}s ({:.1} steps/s); {} organisms, {} food",
        steps as f64 / elapsed,
        world.cells.len(),
        world.food.len()
    );
    println!(
        "integration {:.1}%, contacts {:.1}%, cleanup {:.1}%, food {:.1}%, signals {:.1}%, reproduction {:.1}%",
        timings.integration as f64 / (elapsed * 1.0e9) * 100.0,
        timings.contacts as f64 / (elapsed * 1.0e9) * 100.0,
        timings.cleanup as f64 / (elapsed * 1.0e9) * 100.0,
        timings.food as f64 / (elapsed * 1.0e9) * 100.0,
        timings.signals as f64 / (elapsed * 1.0e9) * 100.0,
        timings.reproduction as f64 / (elapsed * 1.0e9) * 100.0
    );
    println!(
        "contacts breakdown: links {:.1}%, grid {:.1}%, pair search {:.1}%, pair effects {:.1}% of total",
        timings.links as f64 / (elapsed * 1.0e9) * 100.0,
        timings.contact_grid as f64 / (elapsed * 1.0e9) * 100.0,
        timings.contact_search as f64 / (elapsed * 1.0e9) * 100.0,
        timings.contact_effects as f64 / (elapsed * 1.0e9) * 100.0,
    );
    println!(
        "pair effects detail: marking {:.1}%, metadata {:.1}%, pair loop {:.1}%, apply {:.1}% of total",
        timings.contact_marking as f64 / (elapsed * 1.0e9) * 100.0,
        timings.contact_metadata as f64 / (elapsed * 1.0e9) * 100.0,
        timings.contact_pairs as f64 / (elapsed * 1.0e9) * 100.0,
        timings.contact_apply as f64 / (elapsed * 1.0e9) * 100.0,
    );
    println!(
        "signals detail: metabolism {:.1}%, resources {:.1}%, sensing {:.1}% of total",
        timings.metabolism as f64 / (elapsed * 1.0e9) * 100.0,
        timings.resource_exchange as f64 / (elapsed * 1.0e9) * 100.0,
        timings.sensing as f64 / (elapsed * 1.0e9) * 100.0,
    );
    Ok(())
}

fn benchmark_snapshot(arguments: &[String]) -> Result<(), String> {
    if arguments.len() != 5 {
        return Err("Usage: celllab2 --benchmark-snapshot INPUT ITERATIONS CELL_COUNT".into());
    }
    let mut world = native::load(&arguments[2])?;
    let iterations: usize = arguments[3]
        .parse()
        .map_err(|_| "Invalid iteration count")?;
    let requested: usize = arguments[4].parse().map_err(|_| "Invalid cell count")?;
    if world.cells.is_empty() || iterations == 0 {
        return Err("Input needs cells and the iteration count must be positive".into());
    }
    let source = world.cells.clone();
    while world.cells.len() < requested {
        let index = world.cells.len();
        let mut cell = source[index % source.len()].clone();
        cell.runtime_id = model::fresh_cell_id();
        let distance = world.environment.radius * 0.8 * (index as f64 / requested as f64).sqrt();
        let angle = index as f64 * 2.399_963_229_728_653;
        cell.x = distance * angle.cos();
        cell.y = distance * angle.sin();
        cell.links.clear();
        cell.c = 0;
        world.cells.push(cell);
    }
    std::hint::black_box(sim::preview_snapshot(&world));
    let started = std::time::Instant::now();
    let mut chord_count = 0usize;
    for _ in 0..iterations {
        let snapshot = sim::preview_snapshot(&world);
        chord_count += snapshot
            .cells
            .iter()
            .map(|cell| cell.contact_chords.len())
            .sum::<usize>();
        std::hint::black_box(snapshot);
    }
    println!(
        "{iterations} snapshots of {} cells in {:.3}s; {} total chords",
        world.cells.len(),
        started.elapsed().as_secs_f64(),
        chord_count
    );
    Ok(())
}

fn edit_save(arguments: &[String]) -> Result<(), String> {
    if arguments.len() < 4 || (arguments.len() - 4) % 2 != 0 {
        return Err("Usage: celllab2 --edit-save INPUT OUTPUT [--radius R] [--max-cells N] [--max-food N] [--food-count N]".into());
    }
    let mut world = native::load(&arguments[2])?;
    for option in arguments[4..].chunks_exact(2) {
        match option[0].as_str() {
            "--radius" => world.set_radius(option[1].parse().map_err(|_| "Invalid radius")?),
            "--max-cells" => world.set_max_cells(
                option[1]
                    .parse()
                    .map_err(|_| "Invalid maximum organism count")?,
            ),
            "--max-food" => world.set_max_food(
                option[1]
                    .parse()
                    .map_err(|_| "Invalid maximum food count")?,
            ),
            "--food-count" => sim::set_food_count(
                &mut world,
                option[1].parse().map_err(|_| "Invalid food count")?,
            ),
            unknown => return Err(format!("Unknown option {unknown}")),
        }
    }
    native::save(&arguments[3], &world)?;
    println!(
        "Saved {}: {} organisms, {} food, radius {}, limits {}/{}",
        arguments[3],
        world.cells.len(),
        world.food.len(),
        world.environment.radius,
        world.environment.max_cells,
        world.environment.max_food
    );
    Ok(())
}

fn export_genome(arguments: &[String]) -> Result<(), String> {
    if !(4..=5).contains(&arguments.len()) {
        return Err(
            "Usage: celllab2 --export-genome INPUT_PLATE OUTPUT_GENOME [CELL_INDEX]".into(),
        );
    }
    let world = native::load(&arguments[2])?;
    let index: usize = arguments.get(4).map_or(Ok(0), |value| {
        value.parse().map_err(|_| "Invalid organism index")
    })?;
    let cell = world
        .cells
        .get(index)
        .ok_or("Organism index is out of range")?;
    let bytes = legacy::export_genome(&cell.genes)?;
    std::fs::write(&arguments[3], bytes).map_err(|error| error.to_string())?;
    println!(
        "Saved genome from organism {} to {}",
        index + 1,
        arguments[3]
    );
    Ok(())
}
