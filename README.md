# CellLab2

A native Windows reconstruction of Cell Lab, built from the original APK and its bundled plate files. The original APK and reverse engineering tools are kept under `reference/` and `tools/` for local analysis. Those folders are excluded from Git; the files required to build are under `assets/`.

## Run

```powershell
cargo run --release
```

The Windows executable is `target/release/celllab2.exe` after a local build. Local release packages may be placed in `dist/`, which is excluded from Git. The 65 bundled `.substrate` plates are embedded in the executable.
Cargo is configured for two concurrent build jobs in `.cargo/config.toml` to leave CPU capacity for other applications.

## Current controls

- Select a bundled plate or any of the 70 challenges on the left. All challenges and editor controls are available from the start. The Objective column shows each challenge's title, description, hints, tips, and live win conditions. The four tutorial levels use desktop guides with Back and Next steps. Challenge-specific tool and cell-type limits still apply. Completed levels are saved under `%APPDATA%\CellLab2\progress.json`.
- Open an existing `.substrate` or `.celllab2` save with the native Open dialog, by path, or by dragging it onto the window. The toolbar Save button opens a native Save as dialog; the sidebar also accepts a typed destination.
- Play, pause, step, enter any speed of at least 0.1×, or choose Incubate to simulate as fast as the computer allows. Incubate checks controls between short simulation bursts so pause and editing remain responsive.
- The original APK's experiment and challenge music and its placement, selection, removal, save, error, tutorial, and win sounds play through Windows audio. The CREDITS section names the game's creators and Danilov Sound, whom the APK credits for all sound and music. The AUDIO section in the left panel controls music and sound effects separately; choices are saved with challenge progress.
- Change plate radius, maximum cell count, maximum food, and current food count on a loaded plate. The Environment settings dialog also edits the APK's nutrient, light, gravity, friction, mutation, aging, food movement, and available cell type controls. Existing cells and food are preserved when a limit is lowered. Cells outside a reduced radius are removed when simulation resumes.
- Choose Cell, Move, Boost, Remove, or Inspect. Cell places a new organism from the independent synthesizer genome or selects an existing organism; Boost restores a cell or adds a large food particle to empty space. Inspect shows live age, mass, diameter, nitrogen, toxins, injury, lineage tag, and signal levels. Shift-click with a selected organism to place its clone. At the cell cap, placing an organism replaces one randomly chosen organism, as in the APK. Drag a cell with Move to hold it in place while moving it, drag empty space to pan, and scroll to zoom around the pointer. The synthesizer and selected-cell editors cover all 40 gene modes, including division, links, actions, programmed signal inputs, and the APK's Neurocyte oscillator preset. The synthesizer can copy a selected cell's genome and save its own genome to `%APPDATA%\CellLab2\progress.json`.
- The synthesizer starts with the APK's 40-mode Phagocyte genome. All cell types and gene controls are available on free plates. Individual challenges retain their own cell-type and placement limits. Use **Load > Default genome** to start over with the current challenge's first available type without changing organisms already on the plate.
- The landscape layout follows the APK's Substrate or Objective, Microscope, and Genome sections. The Genome panel simulates a separate specimen from the edited genome at a selectable time from 0 to 48 game hours on a one-worker background pool. It uses a repeatable seed, suppresses the APK's plate-only forces and viral infection, and draws cells, links, and active-mode selection rings with the ported APK shaders on the APK's dark preview background. The toolbar uses the APK's original tool and plate icons and its dark green and amber UI palette. WGPU passes translate the APK's plate, food, link, dead cell, all 18 cell body, and eight appendage shaders. Cells use the APK's contact chords to shape overlapping bodies and shade their shared membranes. Photocytes and buoyocytes sample the APK's `small_dots.png` texture; dead cells use its `gauss_bessel_integral.png` lookup. The plate shader also renders in challenges, with the APK's mode colors outside the plate. Link triangles use the APK's rim attachments, Myocyte bend angles, central bar placement, and dash projection. Some Android UI and simulation details still need direct ports.
- Save a `.celllab2` file for the port or a `.substrate` file in the original version 95 format. A backup is made before replacing an existing save, and the original stays in place until the replacement is ready. Both formats preserve organisms and food already present when their limits are lowered.
- The Genome editor follows the APK's open, scrollable Child 1, Child 2, split, color, and cell-type control layout. Signal programming and the Neurocyte oscillator use dialogs. Its preview caches frames across the full 0–48 hour timeline, so scrubbing keeps a visible frame without restarting the simulation for each slider position.
- The Genome editor's Snap checkbox switches its sliders between 24 and 1,200 steps. Cell action sliders use the APK's logarithmic scale, Nutrient priority uses the APK's geometric 0.1–10 scale, and Split ratio moves in the APK's Child 1 direction. Adhesin length and stiffness stay visible but are disabled when Make adhesin is off.

The save editor also has a command line form:

```powershell
celllab2.exe --edit-save input.substrate edited.substrate --radius 1.5 --max-cells 2000 --max-food 6000 --food-count 2500
```

To measure simulation throughput with a limited worker pool:

```powershell
$env:RAYON_NUM_THREADS = '2'
celllab2.exe --benchmark input.substrate 2000
```

The promoted 100,000-cell contact path caches per-cell contact metadata and
uses an x86-64 reciprocal-square-root instruction with a fallback for extreme
distances. Dense plates build render contact chords with contiguous grid bins;
sparse plates retain the hash grid. The `--benchmark-snapshot` command measures
CPU snapshot construction separately from simulation.

## Status

The port opens all 65 bundled plates, runs a multithreaded simulation, and supports native and original format save editing. The simulation covers movement with the APK's fluid drag on cells and food, cell surface friction and its resting threshold, food spawning and decay, food plate-edge handling, feeding with mass-dependent velocity, Devoro nutrient and toxin transfer, nutrient-depletion death, engulfment of smaller cells, Secrocyte food activation, scent signals including secretion mode 6 in wall sensing, protection-aware secretion, and link cutting, APK contact pressure between cells and against the plate, food contact with cell bodies and cilia, light driven growth, energy costs, genome controlled division with inherited organism links, same-step feeding and contact for new daughters, genome mutation governed by plate settings, natural seeding with the APK's origin and Lipocyte reserves, grabbed cells held in place, APK link spring and plate-anchor forces, linked muscle contraction and transfer of mass, nutrients, and toxin, Glueocyte plate anchoring and spike motion, ciliary contact impulses after friction, nearby sensing with analytic light gradients, neural signals that diffuse through links except across Lipocytes, secondary rotation of Photocytes, Buoyocytes, and Gametes, and several cell type interactions. The contact and food grids adapt to large cells on crowded or resized plates so overlapping cells can collide and feed. The movement, division, food, contact, and metabolism passes now follow the APK's broad update order, including removal of engulfed and fused cells on the next step and same-step aging of newly created debris. The sensing distance tables are copied from the original APK. Some original behaviors among the 18 cell types are still being reconstructed; challenge outcomes can differ from the Android game.

Run `cargo test --offline -j 2` for importer, save roundtrip, simulation, and challenge data checks.
Natural deaths inside the plate produce food worth 90% of the cell's remaining mass, including the Lipocyte coating; the Remove tool does not recycle the cell. The new food can be eaten during the same simulation step.
Run the slower, verified challenge checks with `cargo test --offline -j 2 sim::tests:: -- --ignored --nocapture --test-threads 1 --skip coexistence_challenge_can_hold_four_hundred_mixed_cells --skip scavenger_challenge_feeds_on_native_photocyte_remains --skip macrophages_four_can_reach_three_hundred_with_swimmers`. Set `$env:RAYON_NUM_THREADS = '2'` first to limit simulation workers. Eight full challenge proofs cover Algae I–III, Macrophages I–III, Breeding, and Seed. Macrophages III uses a three-mode Phagocyte and Flagellocyte lifecycle; Seed holds 16 Lipocytes for 50 hours without an external food source. A separate live Seed check verifies legal placement, Incubate, the win event, and automatic pause. The regular suite also verifies Colors. Challenge loading resets the bundled plate's cell provenance and clock, while opening it as an ordinary plate preserves them.
Coexistence, Scavenger, and Macrophages IV are not yet verified. The tested Coexistence genome briefly exceeds 400 descendants but has not held that count for 50 hours. The tested Scavenger genome becomes extinct before its 100-hour hold. The inherited Macrophages III genome peaks below Macrophages IV's 300-cell target. These results may reflect genome choices or remaining simulation differences. Run any diagnostic by name with `cargo test --offline -j 2 TEST_NAME -- --ignored --nocapture`.
The slower full save audit is `cargo test --offline -j 2 native::tests::every_bundled_plate_roundtrips_both_formats -- --ignored`. It passed for all 65 bundled plates, comparing environment, food, and every organism and gene field after native and original-format roundtrips. An independent Java `ObjectInputStream` check also decoded an in-place edited `.substrate` with 378 organisms, 1,700 food, radius 1.3, and limits of 120/500. Import repairs two invalid secondary rotation values in the original `hunt` plate; native save writing rejects any remaining non-finite state rather than creating an unreadable file.
