//! Checks on a compiled map, run from the GUI instead of Python scripts.
//!
//! Four analyses, each a port of the script of the same idea in scripts/:
//!
//! - geometry (bspcheck.py): faces that are not planar, not convex or
//!   degenerate, and coplanar pairs face merging left behind.
//! - holes (holecheck.py): rays through the world; where one enters solid or
//!   sky and no drawn face covers the spot, the player sees through the wall.
//! - wpoly (wpolymap.py): how many world faces each leaf's PVS sends to the
//!   renderer, which is what wpoly and the frame rate follow.
//! - hidden faces (hiddenfaces.py): faces drawn for nothing, covered by a
//!   static entity or buried inside world brushes.
//!
//! Every finding carries a position, so the UI can copy it and write a
//! pointfile the editor loads like a leak trail.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::thread;
use std::time::Instant;

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------- settings

/// What to run. Global, saved with the project library.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    pub geometry: bool,
    pub holes: bool,
    pub wpoly: bool,
    pub hidden: bool,
    /// Rays for the hole search.
    pub rays: u32,
    /// Leaves closer than this merge into one wpoly hotspot.
    pub radius: f32,
    /// Analyse every successful compile without asking.
    pub after_compile: bool,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            geometry: true,
            holes: true,
            wpoly: true,
            hidden: true,
            rays: 20000,
            radius: 256.0,
            after_compile: true,
        }
    }
}

// ---------------------------------------------------------------- report

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Ok,
    Info,
    Warn,
    Error,
}

#[derive(Debug, Clone)]
pub struct Finding {
    pub text: String,
    pub pos: [f64; 3],
    /// Relative size, 0..1, for the pointfile marker.
    pub weight: f64,
}

#[derive(Debug, Clone)]
pub struct Section {
    pub title: &'static str,
    /// Suffix of the pointfile, e.g. "holes" for map_holes.pts.
    pub key: &'static str,
    pub level: Level,
    pub summary: String,
    pub advice: &'static str,
    pub findings: Vec<Finding>,
}

#[derive(Debug, Clone)]
pub struct Report {
    pub bsp: PathBuf,
    pub stats: String,
    pub sections: Vec<Section>,
    pub secs: f64,
}

pub enum Msg {
    Progress(String),
    Done(Result<Report, String>),
}

pub struct Job {
    pub rx: Receiver<Msg>,
    cancel: Arc<AtomicBool>,
}

impl Job {
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }
}

/// Runs the chosen analyses on a worker thread.
pub fn start(bsp: PathBuf, map: Option<PathBuf>, prefs: Prefs) -> Job {
    let (tx, rx) = channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let flag = cancel.clone();
    thread::spawn(move || {
        let result = run(&bsp, map.as_deref(), &prefs, &tx, &flag);
        let _ = tx.send(Msg::Done(result));
    });
    Job { rx, cancel }
}

fn run(
    path: &Path,
    map: Option<&Path>,
    prefs: &Prefs,
    tx: &Sender<Msg>,
    cancel: &AtomicBool,
) -> Result<Report, String> {
    let started = Instant::now();
    let _ = tx.send(Msg::Progress("Leyendo el .bsp".to_string()));
    let bsp = Bsp::load(path)?;
    let world = bsp
        .models
        .first()
        .ok_or("el .bsp no tiene modelo del mundo")?;
    let stats = format!(
        "{} caras del mundo, {} hojas, {} entidades brush",
        world.numfaces,
        world.visleafs,
        bsp.models.len().saturating_sub(1)
    );

    let mut sections = Vec::new();
    let stopped = || cancel.load(Ordering::SeqCst);
    if prefs.geometry && !stopped() {
        let _ = tx.send(Msg::Progress("Revisando la geometría".to_string()));
        sections.push(geometry(&bsp));
    }
    if prefs.holes && !stopped() {
        let _ = tx.send(Msg::Progress(format!(
            "Buscando agujeros con {} rayos",
            prefs.rays
        )));
        let targets = match map {
            Some(m) if m.is_file() => match map_world_faces(m) {
                Ok(t) => Some(t),
                Err(e) => {
                    let _ = tx.send(Msg::Progress(format!(
                        "No pude leer el .map ({e}); los rayos salen al azar"
                    )));
                    None
                }
            },
            _ => None,
        };
        sections.push(holes(&bsp, targets.as_deref(), prefs.rays, cancel));
    }
    if prefs.wpoly && !stopped() {
        let _ = tx.send(Msg::Progress("Contando wpoly por hoja".to_string()));
        sections.push(wpoly(&bsp, prefs.radius as f64));
    }
    if prefs.hidden && !stopped() {
        let _ = tx.send(Msg::Progress("Buscando caras ocultas".to_string()));
        sections.extend(hidden(&bsp));
    }
    if stopped() {
        return Err("Análisis cancelado.".to_string());
    }
    Ok(Report {
        bsp: path.to_path_buf(),
        stats,
        sections,
        secs: started.elapsed().as_secs_f64(),
    })
}

/// Writes one star per finding, sized by its weight. J.A.C.K. and Hammer load
/// it with "Load pointfile" like a leak trail.
pub fn write_pointfile(path: &Path, findings: &[Finding]) -> std::io::Result<()> {
    const DIRS: [[f64; 3]; 13] = [
        [1., 0., 0.],
        [0., 1., 0.],
        [0., 0., 1.],
        [1., 1., 0.],
        [1., -1., 0.],
        [1., 0., 1.],
        [1., 0., -1.],
        [0., 1., 1.],
        [0., 1., -1.],
        [1., 1., 1.],
        [1., 1., -1.],
        [1., -1., 1.],
        [1., -1., -1.],
    ];
    let mut out = String::new();
    for f in findings {
        let radius = 16.0 + 48.0 * f.weight.clamp(0.0, 1.0);
        let steps = radius as i32;
        for d in DIRS {
            let l = len(d);
            let mut s = -steps;
            while s <= steps {
                let t = s as f64 / l;
                out.push_str(&format!(
                    "{:.3} {:.3} {:.3}\n",
                    f.pos[0] + d[0] * t,
                    f.pos[1] + d[1] * t,
                    f.pos[2] + d[2] * t
                ));
                s += 2;
            }
        }
    }
    std::fs::write(path, out)
}

// ---------------------------------------------------------------- vectors

type V3 = [f64; 3];

fn dot(a: V3, b: V3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn scale(a: V3, s: f64) -> V3 {
    [a[0] * s, a[1] * s, a[2] * s]
}
fn cross(a: V3, b: V3) -> V3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn len(a: V3) -> f64 {
    dot(a, a).sqrt()
}
fn normalize(a: V3) -> V3 {
    let l = len(a);
    if l > 1e-12 {
        scale(a, 1.0 / l)
    } else {
        [0.0; 3]
    }
}

/// Newell normal: robust when some vertices are collinear, which BSP faces
/// often have after t-junction fixing.
fn newell(pts: &[V3]) -> V3 {
    let mut n = [0.0; 3];
    for i in 0..pts.len() {
        let p = pts[i];
        let q = pts[(i + 1) % pts.len()];
        n[0] += (p[1] - q[1]) * (p[2] + q[2]);
        n[1] += (p[2] - q[2]) * (p[0] + q[0]);
        n[2] += (p[0] - q[0]) * (p[1] + q[1]);
    }
    n
}

fn area(pts: &[V3]) -> f64 {
    len(newell(pts)) / 2.0
}

fn centroid(pts: &[V3]) -> V3 {
    let mut c = [0.0; 3];
    for p in pts {
        c = add(c, *p);
    }
    scale(c, 1.0 / pts.len().max(1) as f64)
}

/// p within tol of the inside of the convex polygon pts (p on its plane).
fn inside(p: V3, pts: &[V3], tol: f64) -> bool {
    let normal = newell(pts);
    for i in 0..pts.len() {
        let a = pts[i];
        let inward = cross(normal, sub(pts[(i + 1) % pts.len()], a));
        let size = len(inward);
        if size > 0.0 && dot(sub(p, a), inward) / size < -tol {
            return false;
        }
    }
    true
}

/// Small deterministic generator, so two runs on the same map agree.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> f64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 11) as f64 / (1u64 << 53) as f64
    }
    fn uniform(&mut self, a: f64, b: f64) -> f64 {
        a + (b - a) * self.next()
    }
    fn gauss(&mut self) -> f64 {
        let u = self.next().max(1e-12);
        let v = self.next();
        (-2.0 * u.ln()).sqrt() * (std::f64::consts::TAU * v).cos()
    }
}

// ---------------------------------------------------------------- bsp

const CONTENTS_EMPTY: i32 = -1;
const CONTENTS_SOLID: i32 = -2;
const CONTENTS_SKY: i32 = -6;
const TEX_SPECIAL: i32 = 1;

struct Plane {
    n: V3,
    d: f64,
}

struct Node {
    plane: usize,
    children: [i32; 2],
}

struct Leaf {
    contents: i32,
    visofs: i32,
    mins: V3,
    maxs: V3,
    firstmark: usize,
    nummarks: usize,
}

struct Face {
    plane: usize,
    side: bool,
    firstedge: i64,
    numedges: usize,
    texinfo: usize,
}

struct TexInfo {
    s: V3,
    t: V3,
    miptex: i32,
    flags: i32,
}

struct Model {
    mins: V3,
    maxs: V3,
    headnode: i32,
    visleafs: usize,
    firstface: usize,
    numfaces: usize,
}

struct Bsp {
    planes: Vec<Plane>,
    verts: Vec<V3>,
    nodes: Vec<Node>,
    texinfo: Vec<TexInfo>,
    faces: Vec<Face>,
    leafs: Vec<Leaf>,
    marks: Vec<usize>,
    edges: Vec<[usize; 2]>,
    surfedges: Vec<i32>,
    models: Vec<Model>,
    texnames: Vec<String>,
    entities: String,
    vis: Vec<u8>,
}

struct Reader<'a> {
    data: &'a [u8],
}

impl Reader<'_> {
    fn i32(&self, o: usize) -> i32 {
        i32::from_le_bytes(self.data[o..o + 4].try_into().unwrap())
    }
    fn u16(&self, o: usize) -> u16 {
        u16::from_le_bytes(self.data[o..o + 2].try_into().unwrap())
    }
    fn i16(&self, o: usize) -> i16 {
        i16::from_le_bytes(self.data[o..o + 2].try_into().unwrap())
    }
    fn f32(&self, o: usize) -> f64 {
        f32::from_le_bytes(self.data[o..o + 4].try_into().unwrap()) as f64
    }
    fn v3(&self, o: usize) -> V3 {
        [self.f32(o), self.f32(o + 4), self.f32(o + 8)]
    }
}

impl Bsp {
    fn load(path: &Path) -> Result<Self, String> {
        let data =
            std::fs::read(path).map_err(|e| format!("no pude leer {}: {e}", path.display()))?;
        if data.len() < 4 + 15 * 8 {
            return Err("el archivo es demasiado corto para ser un .bsp".to_string());
        }
        let r = Reader { data: &data };
        let version = r.i32(0);
        if version != 30 {
            return Err(format!(
                "versión de BSP {version}, se esperaba 30 (GoldSrc)"
            ));
        }
        let mut lumps = [(0usize, 0usize); 15];
        for (i, lump) in lumps.iter_mut().enumerate() {
            let off = r.i32(4 + 8 * i);
            let size = r.i32(8 + 8 * i);
            if off < 0 || size < 0 || off as usize + size as usize > data.len() {
                return Err(format!("el lump {i} sale del archivo: el .bsp está dañado"));
            }
            *lump = (off as usize, size as usize);
        }
        let each =
            |i: usize, step: usize| (0..lumps[i].1 / step).map(move |k| lumps[i].0 + k * step);

        let planes = each(1, 20)
            .map(|o| Plane {
                n: r.v3(o),
                d: r.f32(o + 12),
            })
            .collect();
        let verts = each(3, 12).map(|o| r.v3(o)).collect();
        let nodes = each(5, 24)
            .map(|o| Node {
                plane: r.i32(o) as usize,
                children: [r.i16(o + 4) as i32, r.i16(o + 6) as i32],
            })
            .collect();
        let texinfo = each(6, 40)
            .map(|o| TexInfo {
                s: r.v3(o),
                t: r.v3(o + 16),
                miptex: r.i32(o + 32),
                flags: r.i32(o + 36),
            })
            .collect();
        let faces = each(7, 20)
            .map(|o| Face {
                plane: r.u16(o) as usize,
                side: r.i16(o + 2) != 0,
                firstedge: r.i32(o + 4) as i64,
                numedges: r.i16(o + 8).max(0) as usize,
                texinfo: r.i16(o + 10).max(0) as usize,
            })
            .collect();
        let leafs = each(10, 28)
            .map(|o| Leaf {
                contents: r.i32(o),
                visofs: r.i32(o + 4),
                mins: [
                    r.i16(o + 8) as f64,
                    r.i16(o + 10) as f64,
                    r.i16(o + 12) as f64,
                ],
                maxs: [
                    r.i16(o + 14) as f64,
                    r.i16(o + 16) as f64,
                    r.i16(o + 18) as f64,
                ],
                firstmark: r.u16(o + 20) as usize,
                nummarks: r.u16(o + 22) as usize,
            })
            .collect();
        let marks = each(11, 2).map(|o| r.u16(o) as usize).collect();
        let edges = each(12, 4)
            .map(|o| [r.u16(o) as usize, r.u16(o + 2) as usize])
            .collect();
        let surfedges = each(13, 4).map(|o| r.i32(o)).collect();
        let models = each(14, 64)
            .map(|o| Model {
                mins: r.v3(o),
                maxs: r.v3(o + 12),
                headnode: r.i32(o + 36),
                visleafs: r.i32(o + 52).max(0) as usize,
                firstface: r.i32(o + 56).max(0) as usize,
                numfaces: r.i32(o + 60).max(0) as usize,
            })
            .collect();

        let mut texnames = Vec::new();
        let (toff, tsize) = lumps[2];
        if tsize >= 4 {
            let count = r.i32(toff).max(0) as usize;
            for i in 0..count {
                if toff + 8 + 4 * i > toff + tsize {
                    break;
                }
                let o = r.i32(toff + 4 + 4 * i);
                let start = toff + o.max(0) as usize;
                let name = if o >= 0 && start + 16 <= data.len() {
                    let raw = &data[start..start + 16];
                    let end = raw.iter().position(|b| *b == 0).unwrap_or(16);
                    raw[..end].iter().map(|b| *b as char).collect::<String>()
                } else {
                    String::new()
                };
                texnames.push(name.to_ascii_lowercase());
            }
        }
        let (eo, es) = lumps[0];
        let entities = String::from_utf8_lossy(&data[eo..eo + es]).into_owned();
        let (vo, vs) = lumps[4];
        let vis = data[vo..vo + vs].to_vec();

        Ok(Self {
            planes,
            verts,
            nodes,
            texinfo,
            faces,
            leafs,
            marks,
            edges,
            surfedges,
            models,
            texnames,
            entities,
            vis,
        })
    }

    fn face_points(&self, fi: usize) -> Option<Vec<V3>> {
        let f = &self.faces[fi];
        let mut pts = Vec::with_capacity(f.numedges);
        for k in 0..f.numedges {
            let se = *self.surfedges.get((f.firstedge + k as i64) as usize)?;
            let e = self.edges.get(se.unsigned_abs() as usize)?;
            let v = if se >= 0 { e[0] } else { e[1] };
            pts.push(*self.verts.get(v)?);
        }
        Some(pts)
    }

    fn face_normal(&self, fi: usize) -> V3 {
        let f = &self.faces[fi];
        let n = self.planes[f.plane].n;
        if f.side {
            scale(n, -1.0)
        } else {
            n
        }
    }

    fn texname(&self, fi: usize) -> &str {
        self.texinfo
            .get(self.faces[fi].texinfo)
            .and_then(|t| self.texnames.get(t.miptex.max(0) as usize))
            .map(|s| s.as_str())
            .unwrap_or("")
    }

    fn special(&self, fi: usize) -> bool {
        self.texinfo
            .get(self.faces[fi].texinfo)
            .map(|t| t.flags & TEX_SPECIAL != 0)
            .unwrap_or(true)
    }

    fn contents(&self, head: i32, p: V3) -> i32 {
        let mut n = head;
        let mut guard = 0;
        while n >= 0 && guard < 100_000 {
            let node = &self.nodes[n as usize];
            let pl = &self.planes[node.plane];
            n = node.children[if dot(p, pl.n) - pl.d >= 0.0 { 0 } else { 1 }];
            guard += 1;
        }
        self.leafs
            .get((-n - 1) as usize)
            .map(|l| l.contents)
            .unwrap_or(CONTENTS_SOLID)
    }

    /// Faces listed by a non-solid leaf of the world: the ones the engine can draw.
    fn drawn_faces(&self) -> HashSet<usize> {
        let world = &self.models[0];
        let mut drawn = HashSet::new();
        for leaf in self.leafs.iter().skip(1).take(world.visleafs) {
            if leaf.contents != CONTENTS_SOLID {
                for k in leaf.firstmark..leaf.firstmark + leaf.nummarks {
                    if let Some(m) = self.marks.get(k) {
                        drawn.insert(*m);
                    }
                }
            }
        }
        drawn
    }
}

// ---------------------------------------------------------------- geometry

const ON_EPSILON: f64 = 0.01;
const PLANE_EPSILON: f64 = 0.05;
const MAXEDGES: usize = 48;
const COORD_LIMIT: f64 = 65536.0;
const SUBDIVIDE_SIZE: f64 = 240.0;
const ANGULAR_EPSILON: f64 = 1e-4;

fn geometry(bsp: &Bsp) -> Section {
    let mut findings = Vec::new();
    let mut counts: HashMap<&'static str, usize> = HashMap::new();
    let mut polys: Vec<Option<Vec<V3>>> = Vec::with_capacity(bsp.faces.len());
    let mut problem = |kind: &'static str, fi: usize, pos: V3, findings: &mut Vec<Finding>| {
        *counts.entry(kind).or_default() += 1;
        findings.push(Finding {
            text: format!("cara {fi}: {kind}"),
            pos,
            weight: 0.5,
        });
    };

    for fi in 0..bsp.faces.len() {
        let Some(pts) = bsp.face_points(fi) else {
            problem("índices fuera de rango", fi, [0.0; 3], &mut findings);
            polys.push(None);
            continue;
        };
        if pts.len() < 3 {
            problem("menos de 3 vértices", fi, centroid(&pts), &mut findings);
            polys.push(None);
            continue;
        }
        let c = centroid(&pts);
        if pts.len() > MAXEDGES {
            problem("demasiados vértices", fi, c, &mut findings);
        }
        if bsp.faces[fi].texinfo >= bsp.texinfo.len() {
            problem("texinfo inválido", fi, c, &mut findings);
        }
        let pl = &bsp.planes[bsp.faces[fi].plane];
        if pts
            .iter()
            .any(|p| (dot(*p, pl.n) - pl.d).abs() > PLANE_EPSILON)
        {
            problem("no es plana", fi, c, &mut findings);
        }
        if pts.iter().any(|p| p.iter().any(|v| v.abs() > COORD_LIMIT)) {
            problem("fuera de los límites del motor", fi, c, &mut findings);
        }
        if (0..pts.len()).any(|i| len(sub(pts[i], pts[(i + 1) % pts.len()])) < ON_EPSILON) {
            problem("vértices duplicados", fi, c, &mut findings);
        }
        let normal = bsp.face_normal(fi);
        let mut sign = 0;
        for i in 0..pts.len() {
            let e1 = sub(pts[(i + 1) % pts.len()], pts[i]);
            let e2 = sub(pts[(i + 2) % pts.len()], pts[(i + 1) % pts.len()]);
            let (l1, l2) = (len(e1), len(e2));
            if l1 < ON_EPSILON || l2 < ON_EPSILON {
                continue;
            }
            let turn = dot(cross(e1, e2), normal) / (l1 * l2);
            if turn.abs() < ANGULAR_EPSILON {
                continue;
            }
            let s = if turn > 0.0 { 1 } else { -1 };
            if sign == 0 {
                sign = s;
            } else if s != sign {
                problem("no es convexa", fi, c, &mut findings);
                break;
            }
        }
        if area(&pts) < 1e-4 {
            problem("área cero", fi, c, &mut findings);
        }
        polys.push(Some(pts));
    }

    // Coplanar pairs that could still merge and would still fit the lightmap extent.
    let mut groups: HashMap<(usize, bool, usize), Vec<usize>> = HashMap::new();
    for (fi, f) in bsp.faces.iter().enumerate() {
        groups
            .entry((f.plane, f.side, f.texinfo))
            .or_default()
            .push(fi);
    }
    let mut residual = 0;
    for group in groups.values().filter(|g| g.len() > 1) {
        for x in 0..group.len() {
            for y in x + 1..group.len() {
                let (a, b) = (group[x], group[y]);
                let (Some(pa), Some(pb)) = (&polys[a], &polys[b]) else {
                    continue;
                };
                if let Some(hit) = shares_reversed_edge(pa, pb) {
                    if would_be_convex(pa, pb, hit, bsp.face_normal(a))
                        && fits_subdivision(bsp, bsp.faces[a].texinfo, pa, pb)
                    {
                        residual += 1;
                    }
                }
            }
        }
    }

    let total: usize = counts.values().sum();
    let mut parts: Vec<String> = counts.iter().map(|(k, v)| format!("{v} {k}")).collect();
    parts.sort();
    let summary = if total == 0 {
        format!(
            "{} caras, todas planas, convexas y sin degenerar. Pares que todavía se podrían \
             fusionar: {residual}.",
            bsp.faces.len()
        )
    } else {
        format!(
            "{total} problemas en {} caras: {}. Pares que todavía se podrían fusionar: \
             {residual}.",
            bsp.faces.len(),
            parts.join(", ")
        )
    };
    Section {
        title: "Geometría",
        key: "geometria",
        level: if total > 0 { Level::Error } else { Level::Ok },
        summary,
        advice: "Una cara no plana, no convexa o degenerada es un error del compilador, no \
                 del mapa: guarda el .map y el .bsp y repórtalo.",
        findings,
    }
}

fn shares_reversed_edge(pa: &[V3], pb: &[V3]) -> Option<(usize, usize)> {
    for i in 0..pa.len() {
        let (a1, a2) = (pa[i], pa[(i + 1) % pa.len()]);
        for j in 0..pb.len() {
            let (b1, b2) = (pb[j], pb[(j + 1) % pb.len()]);
            if len(sub(a1, b2)) < ON_EPSILON && len(sub(a2, b1)) < ON_EPSILON {
                return Some((i, j));
            }
        }
    }
    None
}

fn would_be_convex(pa: &[V3], pb: &[V3], (i, j): (usize, usize), normal: V3) -> bool {
    let (na, nb) = (pa.len(), pb.len());
    let (p1, p2) = (pa[i], pa[(i + 1) % na]);
    let n1 = normalize(cross(normal, sub(p1, pa[(i + na - 1) % na])));
    if dot(sub(pb[(j + 2) % nb], p1), n1) > ON_EPSILON {
        return false;
    }
    let n2 = normalize(cross(normal, sub(pa[(i + 2) % na], p2)));
    if dot(sub(pb[(j + nb - 1) % nb], p2), n2) > ON_EPSILON {
        return false;
    }
    na + nb <= MAXEDGES
}

fn fits_subdivision(bsp: &Bsp, ti: usize, pa: &[V3], pb: &[V3]) -> bool {
    let Some(tex) = bsp.texinfo.get(ti) else {
        return false;
    };
    if tex.flags & TEX_SPECIAL != 0 {
        return true;
    }
    [tex.s, tex.t].iter().all(|axis| {
        let vals: Vec<f64> = pa.iter().chain(pb).map(|p| dot(*p, *axis)).collect();
        let lo = vals.iter().cloned().fold(f64::INFINITY, f64::min);
        let hi = vals.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        hi - lo <= SUBDIVIDE_SIZE
    })
}

// ---------------------------------------------------------------- holes

const TOOL_TEXTURES: [&str; 12] = [
    "NULL",
    "SKIP",
    "CLIP",
    "BEVEL",
    "ORIGIN",
    "HINT",
    "SOLIDHINT",
    "BEVELHINT",
    "AAATRIGGER",
    "BOUNDINGBOX",
    "CONTENT",
    "SKY",
];

struct DrawnFace {
    n: V3,
    d: f64,
    pts: Vec<V3>,
    lo: V3,
    hi: V3,
}

enum Hit {
    Start,
    At(V3, usize, usize),
}

fn trace(bsp: &Bsp, num: i32, a: V3, b: V3, depth: u32) -> Option<Hit> {
    if depth > 4096 {
        return None;
    }
    if num < 0 {
        let c = bsp.leafs.get((-num - 1) as usize)?.contents;
        return (c == CONTENTS_SOLID || c == CONTENTS_SKY).then_some(Hit::Start);
    }
    let node = &bsp.nodes[num as usize];
    let pl = &bsp.planes[node.plane];
    let t1 = dot(a, pl.n) - pl.d;
    let t2 = dot(b, pl.n) - pl.d;
    if t1 >= 0.0 && t2 >= 0.0 {
        return trace(bsp, node.children[0], a, b, depth + 1);
    }
    if t1 < 0.0 && t2 < 0.0 {
        return trace(bsp, node.children[1], a, b, depth + 1);
    }
    let side = if t1 >= 0.0 { 0 } else { 1 };
    let frac = t1 / (t1 - t2);
    let mid = add(a, scale(sub(b, a), frac));
    if let Some(hit) = trace(bsp, node.children[side], a, mid, depth + 1) {
        return Some(hit);
    }
    match trace(bsp, node.children[1 - side], mid, b, depth + 1) {
        Some(Hit::Start) => Some(Hit::At(mid, node.plane, side)),
        other => other,
    }
}

fn holes(bsp: &Bsp, targets: Option<&[(Vec<V3>, V3)]>, rays: u32, cancel: &AtomicBool) -> Section {
    let world = &bsp.models[0];
    let head = world.headnode;
    let drawn_set = bsp.drawn_faces();
    // A static func_wall is solid and opaque: nobody stands inside one, and a
    // world face it covers is not a hole even if -autonull removed it.
    let walls = static_walls(bsp);

    // plane -> faces of the world model on it
    let mut by_plane: HashMap<usize, Vec<(usize, Vec<V3>)>> = HashMap::new();
    let mut drawn = Vec::new();
    for fi in world.firstface..world.firstface + world.numfaces {
        let Some(pts) = bsp.face_points(fi) else {
            continue;
        };
        let f = &bsp.faces[fi];
        by_plane
            .entry(f.plane)
            .or_default()
            .push((f.side as usize, pts.clone()));
        if drawn_set.contains(&fi) {
            let pl = &bsp.planes[f.plane];
            let (n, d) = if f.side {
                (scale(pl.n, -1.0), -pl.d)
            } else {
                (pl.n, pl.d)
            };
            let mut lo = [f64::INFINITY; 3];
            let mut hi = [f64::NEG_INFINITY; 3];
            for p in &pts {
                for k in 0..3 {
                    lo[k] = lo[k].min(p[k]);
                    hi[k] = hi[k].max(p[k]);
                }
            }
            drawn.push(DrawnFace { n, d, pts, lo, hi });
        }
    }

    let covered_render = |p: V3, dir: V3| {
        let (reach, tol) = (0.5, 0.1);
        drawn.iter().any(|f| {
            if (0..3).any(|i| p[i] < f.lo[i] - reach || p[i] > f.hi[i] + reach) {
                return false;
            }
            let den = dot(dir, f.n);
            if den >= 0.0 {
                return false;
            }
            let t = (f.d - dot(p, f.n)) / den;
            t.abs() <= reach && inside(add(p, scale(dir, t)), &f.pts, tol)
        })
    };

    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let (mut hits, mut tries) = (0u32, 0u64);
    let mut found: Vec<(bool, V3)> = Vec::new();
    let mut seen = HashSet::new();
    while hits < rays && tries < rays as u64 * 50 {
        tries += 1;
        if tries % 4096 == 0 && cancel.load(Ordering::SeqCst) {
            break;
        }
        let (origin, d) = match targets {
            Some(t) if !t.is_empty() => {
                let (w, n) = &t[(rng.next() * t.len() as f64) as usize % t.len()];
                let weights: Vec<f64> = w.iter().map(|_| rng.next().powi(3)).collect();
                let total: f64 = weights.iter().sum::<f64>().max(1e-12);
                let mut q = [0.0; 3];
                for (p, wt) in w.iter().zip(&weights) {
                    q = add(q, scale(*p, wt / total));
                }
                let back = rng.uniform(0.5, 48.0);
                let origin = add(q, scale(*n, back));
                let d = [
                    -n[0] + rng.uniform(-0.6, 0.6),
                    -n[1] + rng.uniform(-0.6, 0.6),
                    -n[2] + rng.uniform(-0.6, 0.6),
                ];
                (origin, d)
            }
            _ => {
                let origin = [
                    rng.uniform(world.mins[0], world.maxs[0]),
                    rng.uniform(world.mins[1], world.maxs[1]),
                    rng.uniform(world.mins[2], world.maxs[2]),
                ];
                (origin, [rng.gauss(), rng.gauss(), rng.gauss()])
            }
        };
        let size = len(d);
        if size == 0.0
            || bsp.contents(head, origin) != CONTENTS_EMPTY
            || inside_wall(bsp, &walls, origin)
        {
            continue;
        }
        let dir = scale(d, 1.0 / size);
        let Some(Hit::At(p, plane, side)) =
            trace(bsp, head, origin, add(origin, scale(dir, 16384.0)), 0)
        else {
            continue;
        };
        hits += 1;
        let mut facing_away = false;
        let mut ok = false;
        for (fside, pts) in by_plane.get(&plane).map(|v| v.as_slice()).unwrap_or(&[]) {
            if inside(p, pts, 0.1) {
                if *fside == side {
                    ok = true;
                    break;
                }
                facing_away = true;
            }
        }
        if ok || covered_render(p, dir) || inside_wall(bsp, &walls, sub(p, scale(dir, 0.5))) {
            continue;
        }
        let key = (
            facing_away,
            plane,
            (p[0] / 16.0).round() as i64,
            (p[1] / 16.0).round() as i64,
            (p[2] / 16.0).round() as i64,
        );
        if seen.insert(key) {
            found.push((facing_away, p));
        }
    }

    let holes = found.iter().filter(|(b, _)| !b).count();
    let backfaces = found.len() - holes;
    let findings = found
        .iter()
        .map(|(back, p)| Finding {
            text: if *back {
                "cara al revés: se ve la parte de atrás".to_string()
            } else {
                "agujero: se ve a través de la pared".to_string()
            },
            pos: *p,
            weight: if *back { 0.3 } else { 1.0 },
        })
        .collect();
    let mode = if targets.is_some() {
        "apuntando a las caras visibles del .map"
    } else {
        "desde puntos al azar (sin .map, las texturas de herramienta pueden salir como agujeros)"
    };
    Section {
        title: "Agujeros",
        key: "agujeros",
        level: if holes > 0 {
            Level::Error
        } else if backfaces > 0 {
            Level::Warn
        } else {
            Level::Ok
        },
        summary: format!("{hits} rayos {mode}: {holes} agujeros, {backfaces} caras al revés."),
        advice: "Un agujero es un lugar donde el jugador ve a través de una pared que en el \
                 editor está cerrada. Suele venir de vértices fuera de la grilla: acércate a \
                 la coordenada en el editor y revisa ese brush.",
        findings,
    }
}

/// Visible brush faces of worldspawn and func_detail (both end up in the world
/// model), as (polygon, outward normal).
fn map_world_faces(path: &Path) -> Result<Vec<(Vec<V3>, V3)>, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let text: String = bytes.iter().map(|b| *b as char).collect();
    let mut brushes: Vec<Vec<([V3; 3], String)>> = Vec::new();
    let mut ent_brushes: Vec<Vec<([V3; 3], String)>> = Vec::new();
    let mut sides: Vec<([V3; 3], String)> = Vec::new();
    let mut classname = String::new();
    let mut depth = 0;
    for line in text.lines() {
        let s = line.trim();
        if s == "{" {
            depth += 1;
            if depth == 1 {
                ent_brushes.clear();
                classname.clear();
            } else if depth == 2 {
                sides.clear();
            }
            continue;
        }
        if s == "}" {
            if depth == 2 {
                ent_brushes.push(std::mem::take(&mut sides));
            } else if depth == 1 && (classname == "worldspawn" || classname == "func_detail") {
                brushes.append(&mut ent_brushes);
            }
            depth -= 1;
            continue;
        }
        if depth == 1 && s.starts_with("\"classname\"") {
            classname = s.split('"').nth(3).unwrap_or("").to_string();
        }
        if depth == 2 && s.starts_with('(') {
            if let Some(side) = parse_side(s) {
                sides.push(side);
            }
        }
    }

    let mut faces = Vec::new();
    for sides in &brushes {
        let mut planes = Vec::new();
        for (p, _) in sides {
            let n = cross(sub(p[0], p[1]), sub(p[2], p[1]));
            let size = len(n);
            if size == 0.0 {
                planes.clear();
                break;
            }
            let n = scale(n, 1.0 / size);
            planes.push((n, dot(n, p[0])));
        }
        if planes.len() != sides.len() {
            continue;
        }
        for (i, (n, d)) in planes.iter().enumerate() {
            let tex = sides[i].1.to_ascii_uppercase();
            if TOOL_TEXTURES.iter().any(|t| tex.starts_with(t)) {
                continue;
            }
            let mut w = base_winding(*n, *d);
            for (j, (n2, d2)) in planes.iter().enumerate() {
                if i != j {
                    w = chop(&w, *n2, *d2);
                    if w.is_empty() {
                        break;
                    }
                }
            }
            if w.len() >= 3 {
                faces.push((w, *n));
            }
        }
    }
    Ok(faces)
}

fn parse_side(s: &str) -> Option<([V3; 3], String)> {
    let mut pts = [[0.0; 3]; 3];
    let mut rest = s;
    for p in pts.iter_mut() {
        let open = rest.find('(')?;
        let close = rest[open..].find(')')? + open;
        let nums: Vec<f64> = rest[open + 1..close]
            .split_whitespace()
            .filter_map(|v| v.parse().ok())
            .collect();
        if nums.len() != 3 {
            return None;
        }
        *p = [nums[0], nums[1], nums[2]];
        rest = &rest[close + 1..];
    }
    let tex = rest.split_whitespace().next().unwrap_or("").to_string();
    Some((pts, tex))
}

fn base_winding(n: V3, d: f64) -> Vec<V3> {
    let axis = (0..3)
        .max_by(|a, b| n[*a].abs().total_cmp(&n[*b].abs()))
        .unwrap_or(2);
    let mut up = if axis != 2 {
        [0.0, 0.0, 1.0]
    } else {
        [1.0, 0.0, 0.0]
    };
    up = normalize(sub(up, scale(n, dot(up, n))));
    let right = cross(up, n);
    let o = scale(n, d);
    let big = 65536.0;
    [(-1.0, 1.0), (1.0, 1.0), (1.0, -1.0), (-1.0, -1.0)]
        .iter()
        .map(|(sr, su)| add(o, scale(add(scale(right, *sr), scale(up, *su)), big)))
        .collect()
}

/// Keeps the part of w behind the plane (n, d).
fn chop(w: &[V3], n: V3, d: f64) -> Vec<V3> {
    let eps = 1e-5;
    let mut out = Vec::new();
    for i in 0..w.len() {
        let (a, b) = (w[i], w[(i + 1) % w.len()]);
        let (da, db) = (dot(a, n) - d, dot(b, n) - d);
        if da <= eps {
            out.push(a);
        }
        if (da > eps && db < -eps) || (da < -eps && db > eps) {
            out.push(add(a, scale(sub(b, a), da / (da - db))));
        }
    }
    out
}

// ---------------------------------------------------------------- wpoly

fn decompress_vis(vis: &[u8], ofs: i32, numleafs: usize, out: &mut Vec<usize>) {
    out.clear();
    if ofs < 0 || vis.is_empty() {
        out.extend(1..=numleafs);
        return;
    }
    let (mut i, mut leaf) = (ofs as usize, 1usize);
    while leaf <= numleafs && i < vis.len() {
        let b = vis[i];
        i += 1;
        if b == 0 {
            leaf += 8 * *vis.get(i).unwrap_or(&0) as usize;
            i += 1;
            continue;
        }
        for bit in 0..8 {
            if b & (1 << bit) != 0 && leaf + bit <= numleafs {
                out.push(leaf + bit);
            }
        }
        leaf += 8;
    }
}

fn wpoly(bsp: &Bsp, radius: f64) -> Section {
    let world = &bsp.models[0];
    let numleafs = world.visleafs.min(bsp.leafs.len().saturating_sub(1));
    let wf = world.firstface..world.firstface + world.numfaces;
    let leafmarks: Vec<Vec<usize>> = bsp.leafs[..=numleafs]
        .iter()
        .map(|l| {
            (l.firstmark..l.firstmark + l.nummarks)
                .filter_map(|k| bsp.marks.get(k).copied())
                .filter(|f| wf.contains(f))
                .collect()
        })
        .collect();

    // Stamp array instead of a set per leaf: each face remembers the last leaf
    // that counted it.
    let mut stamp = vec![usize::MAX; bsp.faces.len()];
    let mut visible = Vec::new();
    let mut rows = Vec::new();
    for n in 1..=numleafs {
        let leaf = &bsp.leafs[n];
        if leaf.contents == CONTENTS_SOLID {
            continue;
        }
        let mut count = 0usize;
        let mut take = |faces: &[usize], stamp: &mut [usize]| {
            for f in faces {
                if stamp[*f] != n {
                    stamp[*f] = n;
                    count += 1;
                }
            }
        };
        take(&leafmarks[n], &mut stamp);
        decompress_vis(&bsp.vis, leaf.visofs, numleafs, &mut visible);
        for v in &visible {
            take(&leafmarks[*v], &mut stamp);
        }
        let center = scale(add(leaf.mins, leaf.maxs), 0.5);
        rows.push((count, center, n));
    }

    if rows.is_empty() {
        return Section {
            title: "wpoly por zona",
            key: "wpoly",
            level: Level::Warn,
            summary: "El mundo no tiene hojas vacías.".to_string(),
            advice: "",
            findings: Vec::new(),
        };
    }
    let mut counts: Vec<usize> = rows.iter().map(|r| r.0).collect();
    counts.sort_unstable();
    let pick = |q: f64| counts[((q * counts.len() as f64) as usize).min(counts.len() - 1)];
    let mean = counts.iter().sum::<usize>() as f64 / counts.len() as f64;
    let worst = *counts.last().unwrap_or(&1);

    rows.sort_by_key(|r| std::cmp::Reverse(r.0));
    let mut hot: Vec<(usize, V3, usize)> = Vec::new();
    for (count, center, n) in rows {
        if hot.iter().any(|h| len(sub(center, h.1)) < radius) {
            continue;
        }
        hot.push((count, center, n));
        if hot.len() >= 25 {
            break;
        }
    }
    let novis = if bsp.vis.is_empty() {
        " El mapa no tiene VIS: cada hoja ve todo."
    } else {
        ""
    };
    Section {
        title: "wpoly por zona",
        key: "wpoly",
        level: Level::Info,
        summary: format!(
            "Caras del mundo que manda el PVS por hoja: media {mean:.0}, mediana {}, p95 {}, \
             máximo {worst}.{novis}",
            pick(0.5),
            pick(0.95)
        ),
        advice: "Son cotas superiores: el motor además descarta lo que queda fuera de la \
                 pantalla o de espaldas, así que en juego ves más o menos la mitad. Las zonas \
                 de la lista son las que más cuestan: HINT en los pasos estrechos, paredes o \
                 esquinas que corten las visuales largas y NULL en lo que nadie ve.",
        findings: hot
            .iter()
            .map(|(count, center, n)| Finding {
                text: format!("{count} caras en el PVS (hoja {n})"),
                pos: *center,
                weight: *count as f64 / worst.max(1) as f64,
            })
            .collect(),
    }
}

// ---------------------------------------------------------------- hidden

const SKIP_TEXTURES: [&str; 8] = [
    "sky",
    "null",
    "clip",
    "origin",
    "hint",
    "skip",
    "aaatrigger",
    "bevel",
];

/// A func_wall that is always there, always drawn and always solid. Only these
/// hide what is behind them: func_illusionary is not solid, so a player can
/// walk into a bush and look at the floor under it; anything with a
/// targetname or a render mode can be hidden, faded or killed at run time.
/// The same rule as sdHLCSG's -autonull.
struct Wall {
    model: usize,
    origin: V3,
    lo: V3,
    hi: V3,
}

fn static_walls(bsp: &Bsp) -> Vec<Wall> {
    let key = |e: &HashMap<String, String>, k: &str| {
        e.get(k).map(|v| v.trim().to_string()).unwrap_or_default()
    };
    let set = |v: String| !v.is_empty() && v != "0";
    let mut walls = Vec::new();
    for e in parse_entities(&bsp.entities) {
        let Some(mi) = key(&e, "model")
            .strip_prefix('*')
            .and_then(|m| m.parse::<usize>().ok())
        else {
            continue;
        };
        if key(&e, "classname") != "func_wall" || mi == 0 || mi >= bsp.models.len() {
            continue;
        }
        if !key(&e, "targetname").is_empty()
            || set(key(&e, "rendermode"))
            || set(key(&e, "renderfx"))
            || set(key(&e, "zhlt_invisible"))
            || set(key(&e, "zhlt_noclip"))
        {
            continue;
        }
        let m = &bsp.models[mi];
        let drawn = (m.firstface..m.firstface + m.numfaces).any(|fi| {
            let name = bsp.texname(fi);
            !bsp.special(fi) && !SKIP_TEXTURES.iter().any(|t| name.starts_with(t))
        });
        if !drawn {
            continue;
        }
        let mut origin = [0.0; 3];
        for (k, v) in key(&e, "origin").split_whitespace().take(3).enumerate() {
            origin[k] = v.parse().unwrap_or(0.0);
        }
        walls.push(Wall {
            model: mi,
            origin,
            lo: add(m.mins, origin),
            hi: add(m.maxs, origin),
        });
    }
    walls
}

fn inside_wall(bsp: &Bsp, walls: &[Wall], p: V3) -> bool {
    walls.iter().any(|w| {
        (0..3).all(|k| p[k] >= w.lo[k] - 1.0 && p[k] <= w.hi[k] + 1.0)
            && bsp.contents(bsp.models[w.model].headnode, sub(p, w.origin)) == CONTENTS_SOLID
    })
}

fn parse_entities(text: &str) -> Vec<HashMap<String, String>> {
    let mut ents = Vec::new();
    let mut cur: Option<HashMap<String, String>> = None;
    for line in text.lines() {
        let s = line.trim();
        if s.starts_with('{') {
            cur = Some(HashMap::new());
        } else if s.starts_with('}') {
            if let Some(e) = cur.take() {
                ents.push(e);
            }
        } else if let Some(e) = cur.as_mut() {
            let parts: Vec<&str> = s.split('"').collect();
            if parts.len() >= 5 {
                e.insert(parts[1].to_string(), parts[3].to_string());
            }
        }
    }
    ents
}

/// A grid over the whole convex polygon, `margin` units in from its edges, so a
/// face only counts as covered when its corners and borders are covered too.
fn sample_points(pts: &[V3], spacing: f64) -> Vec<V3> {
    let (margin, maxpoints) = (1.0, 1024.0);
    let c = centroid(pts);
    let Some(e) = (0..pts.len())
        .map(|i| sub(pts[(i + 1) % pts.len()], pts[i]))
        .find(|d| dot(*d, *d) > 1e-6)
    else {
        return vec![c];
    };
    let n = newell(pts);
    if len(n) < 1e-6 {
        return vec![c];
    }
    let n = normalize(n);
    let u = normalize(e);
    let v = cross(n, u);
    let poly: Vec<(f64, f64)> = pts.iter().map(|p| (dot(*p, u), dot(*p, v))).collect();
    let (mut smin, mut smax, mut tmin, mut tmax) = (
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
    );
    for (s, t) in &poly {
        smin = smin.min(*s);
        smax = smax.max(*s);
        tmin = tmin.min(*t);
        tmax = tmax.max(*t);
    }
    let step = spacing.max(((smax - smin) * (tmax - tmin) / maxpoints).sqrt());
    let inside2 = |s: f64, t: f64| {
        let mut sign = 0.0;
        for i in 0..poly.len() {
            let ((s0, t0), (s1, t1)) = (poly[i], poly[(i + 1) % poly.len()]);
            let el = (s1 - s0).hypot(t1 - t0);
            if el < 1e-6 {
                continue;
            }
            let cr = ((s1 - s0) * (t - t0) - (t1 - t0) * (s - s0)) / el;
            if cr.abs() < margin {
                return false;
            }
            if sign == 0.0 {
                sign = cr.signum();
            } else if cr.signum() != sign {
                return false;
            }
        }
        true
    };
    let spread = |lo: f64, hi: f64| -> Vec<f64> {
        let (lo, hi) = (lo + margin, hi - margin);
        if hi <= lo {
            return vec![(lo + hi) / 2.0];
        }
        let count = ((hi - lo) / step).ceil() as usize + 1;
        (0..count)
            .map(|i| lo + (hi - lo) * i as f64 / (count - 1) as f64)
            .collect()
    };
    let off = dot(c, n);
    let mut out = Vec::new();
    for s in spread(smin, smax) {
        for t in spread(tmin, tmax) {
            if inside2(s, t) {
                out.push(add(add(scale(u, s), scale(v, t)), scale(n, off)));
            }
        }
    }
    if out.is_empty() {
        out.push(c);
    }
    out
}

fn hidden(bsp: &Bsp) -> Vec<Section> {
    let skip = |fi: usize| {
        let name = bsp.texname(fi);
        bsp.special(fi) || SKIP_TEXTURES.iter().any(|t| name.starts_with(t))
    };

    let covers: Vec<_> = static_walls(bsp)
        .into_iter()
        .map(|w| (w.model, w.origin, w.lo, w.hi, "func_wall".to_string()))
        .collect();

    let world = &bsp.models[0];
    let drawn = bsp.drawn_faces();
    let spacing = 8.0;
    let mut covered = Vec::new();
    let mut never = Vec::new();
    for fi in world.firstface..world.firstface + world.numfaces {
        if skip(fi) {
            continue;
        }
        let Some(pts) = bsp.face_points(fi) else {
            continue;
        };
        if pts.len() < 3 {
            continue;
        }
        if !drawn.contains(&fi) {
            never.push((area(&pts), fi, centroid(&pts), String::new()));
            continue;
        }
        let n = bsp.face_normal(fi);
        let tests: Vec<V3> = sample_points(&pts, spacing)
            .into_iter()
            .map(|p| add(p, scale(n, 0.5)))
            .collect();
        let mut lo = [f64::INFINITY; 3];
        let mut hi = [f64::NEG_INFINITY; 3];
        for p in &tests {
            for k in 0..3 {
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
        }
        for (mi, origin, clo, chi, class) in &covers {
            if (0..3).any(|k| hi[k] < clo[k] || lo[k] > chi[k]) {
                continue;
            }
            let head = bsp.models[*mi].headnode;
            if tests
                .iter()
                .all(|p| bsp.contents(head, sub(*p, *origin)) == CONTENTS_SOLID)
            {
                covered.push((area(&pts), fi, centroid(&pts), class.clone()));
                break;
            }
        }
    }

    let mut buried = Vec::new();
    for (mi, origin, _, _, class) in &covers {
        let m = &bsp.models[*mi];
        for fi in m.firstface..m.firstface + m.numfaces {
            if skip(fi) {
                continue;
            }
            let Some(pts) = bsp.face_points(fi) else {
                continue;
            };
            if pts.len() < 3 {
                continue;
            }
            let pts: Vec<V3> = pts.iter().map(|p| add(*p, *origin)).collect();
            let n = bsp.face_normal(fi);
            if sample_points(&pts, spacing)
                .iter()
                .all(|p| bsp.contents(world.headnode, add(*p, scale(n, 0.5))) == CONTENTS_SOLID)
            {
                buried.push((area(&pts), fi, centroid(&pts), class.clone()));
            }
        }
    }

    let make = |title: &'static str,
                key: &'static str,
                rows: Vec<(f64, usize, V3, String)>,
                what: &str,
                advice: &'static str,
                level: Level| {
        // An empty f64 sum is -0.0, which prints as "-0".
        let total: f64 = rows.iter().map(|r| r.0).sum::<f64>() + 0.0;
        let biggest = rows.iter().map(|r| r.0).fold(1.0, f64::max);
        let mut rows = rows;
        rows.sort_by(|a, b| b.0.total_cmp(&a.0));
        Section {
            title,
            key,
            level: if rows.is_empty() { Level::Ok } else { level },
            summary: format!(
                "{} caras, {:.0} unidades cuadradas {what}.",
                rows.len(),
                total
            ),
            advice,
            findings: rows
                .iter()
                .map(|(a, fi, c, class)| Finding {
                    text: if class.is_empty() {
                        format!("cara {fi}, {} ({a:.0} u²)", bsp.texname(*fi))
                    } else {
                        format!("cara {fi}, {} ({a:.0} u²), {class}", bsp.texname(*fi))
                    },
                    pos: *c,
                    weight: a / biggest,
                })
                .collect(),
        }
    };
    vec![
        make(
            "Caras tapadas por entidades",
            "tapadas",
            covered,
            "de mundo que un func_wall fijo tapa del todo",
            "El motor las dibuja y RAD las ilumina aunque nadie pueda verlas. Activa 'Poner \
             NULL en caras tapadas' en la pestaña CSG y el compilador las quita solo; o \
             ponles NULL a mano, o convierte la entidad en func_detail.",
            Level::Warn,
        ),
        make(
            "Caras enterradas",
            "enterradas",
            buried,
            "de entidades metidas dentro de paredes del mundo",
            "Esas caras de la entidad quedan dentro de un brush del mundo: se dibujan y no se \
             ven. Ponles NULL.",
            Level::Warn,
        ),
        make(
            "Caras que ninguna hoja dibuja",
            "nunca",
            never,
            "que el motor ya no dibuja",
            "No cuestan FPS, solo espacio de lightmap. Si son muchas, revisa si falta NULL \
             en caras que quedaron fuera del mapa.",
            Level::Info,
        ),
    ]
}
