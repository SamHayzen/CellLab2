use std::io::Read;

use flate2::read::GzDecoder;
use flate2::{Compression, write::GzEncoder};
use std::io::Write;

use crate::model::{Cell, Environment, Food, Gene, GeneInput, Link, World};

const STREAM_HEADER: [u8; 4] = [0xac, 0xed, 0x00, 0x05];

struct JavaBlocks<'a> {
    bytes: &'a [u8],
    position: usize,
    remaining: usize,
}

impl<'a> JavaBlocks<'a> {
    fn new(bytes: &'a [u8]) -> Result<Self, String> {
        if !bytes.starts_with(&STREAM_HEADER) {
            return Err("missing Java object stream header".into());
        }
        Ok(Self {
            bytes,
            position: 4,
            remaining: 0,
        })
    }

    fn refill(&mut self) -> Result<(), String> {
        if self.remaining != 0 {
            return Ok(());
        }
        let tag = *self
            .bytes
            .get(self.position)
            .ok_or("unexpected end of stream")?;
        self.position += 1;
        self.remaining = match tag {
            0x77 => {
                let size = *self.bytes.get(self.position).ok_or("missing block size")? as usize;
                self.position += 1;
                size
            }
            0x7a => {
                let size = self.raw::<4>()?;
                u32::from_be_bytes(size) as usize
            }
            _ => return Err(format!("expected Java primitive block, got 0x{tag:02x}")),
        };
        if self.remaining > self.bytes.len() - self.position {
            return Err("Java primitive block exceeds file".into());
        }
        Ok(())
    }

    fn raw<const N: usize>(&mut self) -> Result<[u8; N], String> {
        let end = self
            .position
            .checked_add(N)
            .ok_or("stream offset overflow")?;
        let slice = self
            .bytes
            .get(self.position..end)
            .ok_or("truncated stream")?;
        self.position = end;
        Ok(slice.try_into().unwrap())
    }

    fn data<const N: usize>(&mut self) -> Result<[u8; N], String> {
        let mut result = [0; N];
        let mut written = 0;
        while written < N {
            self.refill()?;
            if self.remaining == 0 {
                continue;
            }
            let amount = (N - written).min(self.remaining);
            result[written..written + amount]
                .copy_from_slice(&self.bytes[self.position..self.position + amount]);
            self.position += amount;
            self.remaining -= amount;
            written += amount;
        }
        Ok(result)
    }

    fn i16(&mut self) -> Result<i16, String> {
        Ok(i16::from_be_bytes(self.data()?))
    }
    fn i32(&mut self) -> Result<i32, String> {
        Ok(i32::from_be_bytes(self.data()?))
    }
    fn f32(&mut self) -> Result<f32, String> {
        Ok(f32::from_bits(u32::from_be_bytes(self.data()?)))
    }
    fn f64(&mut self) -> Result<f64, String> {
        Ok(f64::from_bits(u64::from_be_bytes(self.data()?)))
    }
    fn boolean(&mut self) -> Result<bool, String> {
        Ok(self.data::<1>()?[0] != 0)
    }
}

fn count(reader: &mut JavaBlocks<'_>, maximum: usize) -> Result<usize, String> {
    let number = reader.i32()?;
    if number < 0 || number as usize > maximum {
        return Err(format!("invalid collection length {number}"));
    }
    Ok(number as usize)
}

fn environment(reader: &mut JavaBlocks<'_>) -> Result<Environment, String> {
    let version = reader.i32()?;
    if !(1..=95).contains(&version) {
        return Err(format!("unsupported environment version {version}"));
    }
    let mut a = reader.f64()?;
    if version == 1 {
        a *= 0.15;
    }
    let b = if version > 1 { reader.f64()? } else { 0.15 };
    let c = reader.f64()?;
    let d = reader.f64()?;
    let e = reader.f64()?;
    let f = reader.f64()?;
    if version <= 23 {
        let _ = reader.f64()?;
    }
    let g = reader.f64()?;
    let types = count(reader, 64)?;
    let mut enabled_types = (0..types)
        .map(|_| reader.boolean())
        .collect::<Result<Vec<_>, _>>()?;
    // Older APK saves have fewer cell types. Their missing entries are
    // disabled; normalize to the current 18-slot environment on import.
    enabled_types.resize(18, false);
    let w = reader.boolean()?;
    let i = reader.f64()?;
    let h = reader.f64()?;
    let j = reader.f64()?;
    let x = reader.boolean()?;
    let z = if version > 20 {
        reader.f64()?
    } else {
        if reader.boolean()? { 1.0 } else { 0.0 }
    };
    let (max_cells, max_food, radius) = if version >= 3 {
        (
            count(reader, 10_000_000)?,
            count(reader, 100_000_000)?,
            reader.f64()?,
        )
    } else {
        (800, 3500, 1.0)
    };
    let (m, l, u) = if version >= 9 {
        (reader.f64()?, reader.f64()?, reader.boolean()?)
    } else {
        (0.0, 0.0, false)
    };
    let n = if version >= 10 { reader.f32()? } else { 0.3 };
    let (v, p, q) = if version >= 14 {
        (reader.boolean()?, reader.f64()?, reader.f64()?)
    } else {
        (true, 0.0, 0.01)
    };
    let (y, o) = if version >= 22 {
        (reader.boolean()?, reader.f32()?)
    } else {
        (false, 0.0)
    };
    Ok(Environment {
        a,
        b,
        c,
        d,
        e,
        f,
        g,
        h,
        i,
        j,
        radius,
        l,
        m,
        n,
        o,
        p,
        q,
        max_cells,
        max_food,
        enabled_types,
        u,
        v,
        w,
        x,
        y,
        z,
    })
}

fn link(reader: &mut JavaBlocks<'_>, version: i32) -> Result<Link, String> {
    let target = reader.i32()?;
    let c = reader.f64()?;
    let d = reader.f64()?;
    let b = reader.boolean()?;
    let e = reader.f64()?;
    let f = reader.f64()?;
    let k = if version >= 7 {
        reader.f32()?
    } else if version >= 4 {
        reader.f64()? as f32
    } else {
        0.0
    };
    let l = if version >= 16 { reader.f32()? } else { 0.0 };
    Ok(Link {
        target,
        c,
        d,
        b,
        e,
        f,
        k,
        l,
    })
}

fn gene(reader: &mut JavaBlocks<'_>, cell_version: i32) -> Result<Gene, String> {
    let version = if cell_version > 2 { reader.i32()? } else { 1 };
    if !(1..=95).contains(&version) {
        return Err(format!("unsupported gene version {version}"));
    }
    let color = [reader.f32()?, reader.f32()?, reader.f32()?];
    let mut floats = [0.0; 7];
    for value in &mut floats[..6] {
        *value = if version < 7 {
            reader.f64()? as f32
        } else {
            reader.f32()?
        };
    }
    let i = reader.i32()?;
    let j = reader.i32()?;
    let mut flags = [false; 7];
    for flag in &mut flags[..3] {
        *flag = reader.boolean()?;
    }
    let cell_type = reader.i32()?;
    let k = reader.i32()?;
    flags[3] = reader.boolean()?;
    let mut old_input = 0.0;
    let mut old_modes = [0, 0];
    if version < 9 {
        old_input = if version < 7 {
            reader.f64()? as f32
        } else {
            reader.f32()?
        };
        if version >= 3 {
            old_modes = [reader.i32()?, reader.i32()?];
        }
    }
    for flag in &mut flags[4..] {
        *flag = if version >= 4 {
            reader.boolean()?
        } else {
            false
        };
    }
    floats[6] = if version >= 4 {
        if version < 7 {
            reader.f64()? as f32
        } else {
            let h = reader.f32()?;
            if version < 16 { h * 35.0 } else { h }
        }
    } else {
        0.0
    };
    let older_threshold = if version == 8 { reader.f32()? } else { 0.4 };
    let mut inputs = [GeneInput::default(); 11];
    let mut output_modes = [0; 11];
    let mut output_values = [0.0; 7];
    if version < 9 {
        inputs[0].b = (older_threshold * 2.0) - 1.0;
        inputs[1].b = (old_input * 2.0) - 1.0;
        output_modes[0] = old_modes[0];
        output_modes[1] = old_modes[1];
        output_modes[9] = 19;
        output_values[6] = 1.0;
    } else {
        for input in &mut inputs {
            let source = reader.i16()?;
            let mode = if version >= 17 {
                reader.i16()?
            } else if source < 4 {
                1
            } else {
                2
            };
            *input = GeneInput {
                source,
                mode,
                a: reader.f32()?,
                b: reader.f32()?,
                c: reader.f32()?,
            };
        }
        for (index, value) in output_modes.iter_mut().enumerate() {
            *value = if version == 10 && index == 4 {
                1
            } else if version < 15 && index == 9 {
                18
            } else if version < 20 && index == 10 {
                0
            } else {
                reader.i32()?
            };
        }
        for (index, value) in output_values.iter_mut().enumerate() {
            *value = if version < 14 && index == 6 {
                1.0
            } else if version < 16 && index == 5 {
                0.0
            } else {
                reader.f32()?
            };
        }
    }
    Ok(Gene {
        color,
        floats,
        i,
        j,
        k,
        flags,
        cell_type,
        inputs,
        output_modes,
        output_values,
    })
}

fn cell(reader: &mut JavaBlocks<'_>) -> Result<Cell, String> {
    let version = reader.i32()?;
    if !(2..=95).contains(&version) {
        return Err(format!("unsupported cell version {version}"));
    }
    let x = reader.f64()?;
    let y = reader.f64()?;
    let mut values = [0.0; 9];
    for value in &mut values {
        *value = reader.f64()?;
    }
    // The bundled hunt plate contains NaN in a cell's secondary rotation
    // fields. Keep imported saves usable in the simulation and JSON format.
    for index in [1, 5] {
        if !values[index].is_finite() {
            values[index] = 0.0;
        }
    }
    let link_count = count(reader, 1_000)?;
    let mut links = (0..link_count)
        .map(|_| link(reader, version))
        .collect::<Result<Vec<_>, _>>()?;
    links.resize_with(20, default_link);
    let c = reader.i32()?;
    let x_flag = reader.boolean()?;
    let color = [reader.f32()?, reader.f32()?, reader.f32()?];
    let gene_count = count(reader, 1_000)?;
    let mut genes = (0..gene_count)
        .map(|_| gene(reader, version))
        .collect::<Result<Vec<_>, _>>()?;
    genes.truncate(40);
    let fallback = genes.first().cloned().unwrap_or_else(default_gene);
    while genes.len() < 40 {
        let mut copy = fallback.clone();
        copy.flags[4] = false;
        genes.push(copy);
    }
    let mode = reader.i32()?;
    let e = reader.i32()?;
    let f = reader.i32()?;
    let tuvp = [reader.f64()?, reader.f64()?, reader.f64()?, reader.f64()?];
    let y_flag = if version >= 4 {
        reader.boolean()?
    } else {
        false
    };
    let mut u = [0.01; 4];
    let mut w = [0.01; 4];
    if version >= 9 {
        for index in 0..4 {
            u[index] = reader.f32()?;
            w[index] = reader.f32()?;
        }
    }
    let mut tail = [0.0; 6];
    if version >= 9 {
        for value in &mut tail[..5] {
            *value = reader.f32()?;
        }
        if version > 11 {
            tail[5] = reader.f32()?;
        }
    }
    let g = if version >= 15 { reader.i32()? } else { 0 };
    let t = if version >= 19 { reader.i32()? } else { -1 };
    let q = if version >= 23 { reader.f64()? } else { 0.0 };
    Ok(Cell {
        runtime_id: crate::model::fresh_cell_id(),
        x,
        y,
        values,
        links,
        genes,
        c,
        x_flag,
        color,
        mode,
        e,
        f,
        tuvp,
        y_flag,
        u,
        w,
        tail,
        g,
        t,
        q,
        resting: false,
        visual_phase: 0.0,
        visual_phase2: 0.0,
        mutation_ticks: 0,
    })
}

pub fn import(bytes: &[u8]) -> Result<World, String> {
    let mut reader = JavaBlocks::new(bytes)?;
    let version = reader.i32()?;
    if !(1..=95).contains(&version) {
        return Err(format!("unsupported substrate version {version}"));
    }
    let time = reader.f64()?;
    let cell_count = count(&mut reader, 10_000_000)?;
    let environment = environment(&mut reader)?;
    let (light_angle, cells, food) = if version <= 12 {
        parse_body(&mut reader, version, cell_count)?
    } else {
        if reader.remaining != 0 {
            return Err("unexpected bytes after substrate header".into());
        }
        let mut payload = Vec::new();
        // Some bundled substrates omit the gzip trailer even though their
        // decompressed Java stream is complete. Decode the available body and
        // let the field parser decide whether the save itself is complete.
        let decompression_error = GzDecoder::new(&bytes[reader.position..])
            .read_to_end(&mut payload)
            .err();
        let mut body = JavaBlocks::new(&payload)?;
        parse_body(&mut body, version, cell_count).map_err(|error| match decompression_error {
            Some(decompression_error) => format!("{error}; gzip: {decompression_error}"),
            None => error,
        })?
    };
    Ok(World {
        source_version: Some(version),
        time,
        light_angle,
        environment,
        cells,
        food,
        dead: Vec::new(),
    })
}

fn parse_body(
    reader: &mut JavaBlocks<'_>,
    version: i32,
    cell_count: usize,
) -> Result<(f64, Vec<Cell>, Vec<Food>), String> {
    let light_angle = if version > 23 {
        reader.f64()?
    } else {
        std::f64::consts::FRAC_PI_2
    };
    let cells = (0..cell_count)
        .map(|_| cell(reader))
        .collect::<Result<Vec<_>, _>>()?;
    let food_count = count(reader, 100_000_000)?;
    let food = (0..food_count)
        .map(|_| {
            Ok(Food {
                x: if version >= 8 {
                    reader.f32()?
                } else {
                    reader.f64()? as f32
                },
                y: if version >= 8 {
                    reader.f32()?
                } else {
                    reader.f64()? as f32
                },
                energy: if version >= 8 {
                    reader.f32()?
                } else {
                    reader.f64()? as f32
                },
                vx: if version >= 22 { reader.f32()? } else { 0.0 },
                vy: if version >= 22 { reader.f32()? } else { 0.0 },
                state: if version >= 22 { reader.f32()? } else { 0.0 },
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok((light_angle, cells, food))
}

struct BlockWriter {
    data: Vec<u8>,
}

impl BlockWriter {
    fn new() -> Self {
        Self { data: Vec::new() }
    }
    fn i16(&mut self, value: i16) {
        self.data.extend_from_slice(&value.to_be_bytes());
    }
    fn i32(&mut self, value: i32) {
        self.data.extend_from_slice(&value.to_be_bytes());
    }
    fn f32(&mut self, value: f32) {
        self.data.extend_from_slice(&value.to_bits().to_be_bytes());
    }
    fn f64(&mut self, value: f64) {
        self.data.extend_from_slice(&value.to_bits().to_be_bytes());
    }
    fn boolean(&mut self, value: bool) {
        self.data.push(u8::from(value));
    }
    fn finish(self) -> Vec<u8> {
        let mut result = STREAM_HEADER.to_vec();
        for chunk in self.data.chunks(1024) {
            if chunk.len() <= 255 {
                result.push(0x77);
                result.push(chunk.len() as u8);
            } else {
                result.push(0x7a);
                result.extend_from_slice(&(chunk.len() as u32).to_be_bytes());
            }
            result.extend_from_slice(chunk);
        }
        result
    }
}

fn legacy_environment(writer: &mut BlockWriter, env: &Environment) {
    writer.i32(95);
    for value in [env.a, env.b, env.c, env.d, env.e, env.f, env.g] {
        writer.f64(value);
    }
    writer.i32(18);
    for index in 0..18 {
        writer.boolean(env.enabled_types.get(index).copied().unwrap_or(false));
    }
    writer.boolean(env.w);
    for value in [env.i, env.h, env.j] {
        writer.f64(value);
    }
    writer.boolean(env.x);
    writer.f64(env.z);
    writer.i32(env.max_cells as i32);
    writer.i32(env.max_food as i32);
    writer.f64(env.radius);
    writer.f64(env.m);
    writer.f64(env.l);
    writer.boolean(env.u);
    writer.f32(env.n);
    writer.boolean(env.v);
    writer.f64(env.p);
    writer.f64(env.q);
    writer.boolean(env.y);
    writer.f32(env.o);
}

fn legacy_link(writer: &mut BlockWriter, link: &Link) {
    writer.i32(link.target);
    writer.f64(link.c);
    writer.f64(link.d);
    writer.boolean(link.b);
    writer.f64(link.e);
    writer.f64(link.f);
    writer.f32(link.k);
    writer.f32(link.l);
}

fn default_link() -> Link {
    Link {
        target: -1,
        c: 0.0,
        d: 0.0,
        b: false,
        e: 0.0,
        f: 0.0,
        k: 0.0,
        l: 0.0,
    }
}

fn default_gene() -> Gene {
    Gene {
        color: [0.5; 3],
        floats: [0.0; 7],
        i: 0,
        j: 0,
        k: 0,
        flags: [false; 7],
        cell_type: 0,
        inputs: [GeneInput::default(); 11],
        output_modes: [0; 11],
        output_values: [0.0; 7],
    }
}

fn legacy_gene(writer: &mut BlockWriter, gene: &Gene) {
    writer.i32(95);
    for color in gene.color {
        writer.f32(color);
    }
    for value in &gene.floats[..6] {
        writer.f32(*value);
    }
    writer.i32(gene.i);
    writer.i32(gene.j);
    for flag in &gene.flags[..3] {
        writer.boolean(*flag);
    }
    writer.i32(gene.cell_type);
    writer.i32(gene.k);
    writer.boolean(gene.flags[3]);
    for flag in &gene.flags[4..] {
        writer.boolean(*flag);
    }
    writer.f32(gene.floats[6]);
    for index in 0..11 {
        let input = gene.inputs.get(index).cloned().unwrap_or(GeneInput {
            source: 0,
            mode: 0,
            a: 0.0,
            b: 0.0,
            c: 0.0,
        });
        writer.i16(input.source);
        writer.i16(input.mode);
        writer.f32(input.a);
        writer.f32(input.b);
        writer.f32(input.c);
    }
    for value in gene.output_modes {
        writer.i32(value);
    }
    for value in gene.output_values {
        writer.f32(value);
    }
}

/// Read a standalone Android Cell Lab `.genome` file.
pub fn import_genome(bytes: &[u8]) -> Result<Vec<Gene>, String> {
    let mut reader = JavaBlocks::new(bytes)?;
    let version = reader.i32()?;
    if !(1..=95).contains(&version) {
        return Err(format!("unsupported genome version {version}"));
    }
    let count = if version <= 3 {
        15
    } else if version <= 17 {
        20
    } else {
        40
    };
    let mut genes = (0..count)
        .map(|_| gene(&mut reader, version))
        .collect::<Result<Vec<_>, _>>()?;
    let fallback = genes[0].clone();
    while genes.len() < 40 {
        let mut copy = fallback.clone();
        copy.flags[4] = false;
        genes.push(copy);
    }
    let mut has_initial = false;
    for gene in &mut genes {
        if gene.flags[4] {
            gene.flags[4] = !has_initial;
            has_initial = true;
        }
    }
    if !has_initial {
        genes[0].flags[4] = true;
    }
    Ok(genes)
}

/// Write the same Java primitive stream used by the APK's genome editor.
pub fn export_genome(genes: &[Gene]) -> Result<Vec<u8>, String> {
    let fallback = genes.first().ok_or("cannot save an empty genome")?;
    let mut writer = BlockWriter::new();
    writer.i32(95);
    for index in 0..40 {
        legacy_gene(&mut writer, genes.get(index).unwrap_or(fallback));
    }
    Ok(writer.finish())
}

#[cfg(test)]
mod genome_tests {
    use super::*;

    #[test]
    fn original_genome_round_trip_keeps_all_gene_fields() {
        let mut genes = crate::model::default_player_genome();
        genes[7].cell_type = 3;
        genes[7].floats[3] = 0.735;
        genes[7].inputs[2].source = 5;
        genes[7].inputs[2].mode = 2;
        genes[7].inputs[2].a = -0.25;
        genes[7].output_modes[3] = 11;
        genes[7].output_values[3] = 0.8;
        let bytes = export_genome(&genes).unwrap();
        assert!(bytes.starts_with(&STREAM_HEADER));
        let restored = import_genome(&bytes).unwrap();
        assert_eq!(
            serde_json::to_value(&genes).unwrap(),
            serde_json::to_value(&restored).unwrap()
        );
    }

    #[test]
    fn truncated_genome_is_rejected() {
        let bytes = export_genome(&crate::model::default_player_genome()).unwrap();
        assert!(import_genome(&bytes[..bytes.len() - 1]).is_err());
    }
}

fn legacy_cell(writer: &mut BlockWriter, cell: &Cell) {
    writer.i32(95);
    writer.f64(cell.x);
    writer.f64(cell.y);
    for value in cell.values {
        writer.f64(value);
    }
    writer.i32(20);
    for index in 0..20 {
        legacy_link(
            writer,
            &cell.links.get(index).cloned().unwrap_or_else(default_link),
        );
    }
    writer.i32(cell.c.clamp(0, 20));
    writer.boolean(cell.x_flag);
    for color in cell.color {
        writer.f32(color);
    }
    writer.i32(40);
    let fallback = cell.genes.first().cloned().unwrap_or_else(default_gene);
    for index in 0..40 {
        legacy_gene(writer, cell.genes.get(index).unwrap_or(&fallback));
    }
    writer.i32(cell.mode.clamp(0, 39));
    writer.i32(cell.e);
    writer.i32(cell.f);
    for value in cell.tuvp {
        writer.f64(value);
    }
    writer.boolean(cell.y_flag);
    for index in 0..4 {
        writer.f32(cell.u[index]);
        writer.f32(cell.w[index]);
    }
    for value in cell.tail {
        writer.f32(value);
    }
    writer.i32(cell.g);
    writer.i32(cell.t);
    writer.f64(cell.q);
}

pub fn export(world: &World) -> Result<Vec<u8>, String> {
    if world.environment.radius <= 0.0 || !world.environment.radius.is_finite() {
        return Err("Original-format save requires a positive finite plate radius".into());
    }
    for (name, value) in [
        ("organism", world.cells.len()),
        ("food", world.food.len()),
        ("maximum organisms", world.environment.max_cells),
        ("maximum food", world.environment.max_food),
    ] {
        if value > i32::MAX as usize {
            return Err(format!(
                "{name} count is too large for original save format"
            ));
        }
    }
    let mut header = BlockWriter::new();
    header.i32(95);
    header.f64(world.time);
    header.i32(world.cells.len() as i32);
    legacy_environment(&mut header, &world.environment);
    let mut body = BlockWriter::new();
    body.f64(world.light_angle);
    for cell in &world.cells {
        legacy_cell(&mut body, cell);
    }
    body.i32(world.food.len() as i32);
    for food in &world.food {
        for value in [food.x, food.y, food.energy, food.vx, food.vy, food.state] {
            body.f32(value);
        }
    }
    let mut bytes = header.finish();
    let mut compressor = GzEncoder::new(Vec::new(), Compression::fast());
    compressor
        .write_all(&body.finish())
        .map_err(|e| e.to_string())?;
    bytes.extend_from_slice(&compressor.finish().map_err(|e| e.to_string())?);
    Ok(bytes)
}
