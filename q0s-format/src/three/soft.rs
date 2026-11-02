//! The software 3D renderer: z-buffered triangles with perspective-correct
//! textures and per-pixel light, spread over the machine's cores by bands
//! of rows. Exports and the CPU paths draw 3D with it; the GPU renderer
//! follows the same rules.

use super::draw::{
    shade_with, shadow_views, DrawItem, Frame3d, Image, ShadowView, Surface, View3d,
};
use super::math::V3;
use super::model::{Shading, Texture};
use crate::transform::Affine;

/// Draws `frame` into premultiplied RGBA pixels of its output size,
/// supersampled `ss` × `ss` for smooth edges.
pub fn render(frame: &Frame3d, ss: u32) -> Vec<u8> {
    let [w, h] = frame.view.output;
    if w == 0 || h == 0 {
        return Vec::new();
    }
    let ss = ss.clamp(1, 4);
    if ss == 1 {
        return render_once(frame);
    }
    let mut big = frame.clone();
    big.view.output = [w * ss, h * ss];
    big.view.to_output =
        Affine::compose(Affine::from_uniform_scale(ss as f32), frame.view.to_output);
    let pixels = render_once(&big);
    downsample(&pixels, w, h, ss)
}

fn downsample(pixels: &[u8], w: u32, h: u32, ss: u32) -> Vec<u8> {
    let big_w = (w * ss) as usize;
    let mut out = vec![0u8; (w * h * 4) as usize];
    let count = ss * ss;
    for y in 0..h as usize {
        for x in 0..w as usize {
            let mut sum = [0u32; 4];
            for sy in 0..ss as usize {
                let row = (y * ss as usize + sy) * big_w;
                for sx in 0..ss as usize {
                    let i = (row + x * ss as usize + sx) * 4;
                    for c in 0..4 {
                        sum[c] += u32::from(pixels[i + c]);
                    }
                }
            }
            let o = (y * w as usize + x) * 4;
            for c in 0..4 {
                out[o + c] = ((sum[c] + count / 2) / count) as u8;
            }
        }
    }
    out
}

struct Sampler<'a> {
    width: u32,
    height: u32,
    data: &'a [u8],
    premultiplied: bool,
    repeat: bool,
    nearest: bool,
}

impl Sampler<'_> {
    fn texel(&self, x: i64, y: i64) -> [f32; 4] {
        let (w, h) = (i64::from(self.width), i64::from(self.height));
        let (x, y) = if self.repeat {
            (x.rem_euclid(w), y.rem_euclid(h))
        } else {
            (x.clamp(0, w - 1), y.clamp(0, h - 1))
        };
        let i = ((y * w + x) * 4) as usize;
        let px = &self.data[i..i + 4];
        let a = f32::from(px[3]) / 255.0;
        let k = if self.premultiplied {
            1.0 / 255.0
        } else {
            a / 255.0
        };
        [
            f32::from(px[0]) * k,
            f32::from(px[1]) * k,
            f32::from(px[2]) * k,
            a,
        ]
    }

    /// Premultiplied colour at texture position `uv`.
    fn sample(&self, uv: [f32; 2]) -> [f32; 4] {
        if self.width == 0 || self.height == 0 {
            return [1.0; 4];
        }
        let fx = uv[0] * self.width as f32 - 0.5;
        let fy = uv[1] * self.height as f32 - 0.5;
        if self.nearest {
            return self.texel((fx + 0.5).floor() as i64, (fy + 0.5).floor() as i64);
        }
        let (x0, y0) = (fx.floor(), fy.floor());
        let (tx, ty) = (fx - x0, fy - y0);
        let (x0, y0) = (x0 as i64, y0 as i64);
        let a = self.texel(x0, y0);
        let b = self.texel(x0 + 1, y0);
        let c = self.texel(x0, y0 + 1);
        let d = self.texel(x0 + 1, y0 + 1);
        let mut out = [0.0; 4];
        for i in 0..4 {
            let top = a[i] + (b[i] - a[i]) * tx;
            let bottom = c[i] + (d[i] - c[i]) * tx;
            out[i] = top + (bottom - top) * ty;
        }
        out
    }
}

/// How the pixels of one group of triangles get their colour.
struct Paint<'a> {
    /// Premultiplied.
    base: [f32; 4],
    texture: Option<Sampler<'a>>,
    surface: Surface,
    lit: bool,
    cutoff: Option<f32>,
    opacity: f32,
    /// Vertex colours multiply the base.
    vertex_colors: bool,
    blends: bool,
    /// Hidden behind what is nearer; layered drawings are not.
    depth_test: bool,
}

#[derive(Clone, Copy)]
struct Vert {
    view: V3,
    world: V3,
    normal: V3,
    uv: [f32; 2],
    color: [f32; 4],
}

fn lerp_vert(a: &Vert, b: &Vert, t: f32) -> Vert {
    let mix4 = |x: [f32; 4], y: [f32; 4]| std::array::from_fn(|i| x[i] + (y[i] - x[i]) * t);
    Vert {
        view: a.view.lerp(b.view, t),
        world: a.world.lerp(b.world, t),
        normal: a.normal.lerp(b.normal, t),
        uv: [
            a.uv[0] + (b.uv[0] - a.uv[0]) * t,
            a.uv[1] + (b.uv[1] - a.uv[1]) * t,
        ],
        color: mix4(a.color, b.color),
    }
}

struct Tri {
    /// Output x, y, view depth and 1/w per corner.
    screen: [[f32; 4]; 3],
    verts: [Vert; 3],
    paint: u32,
    /// Pass from [`super::draw::draw_passes`].
    order: u32,
    min_y: i32,
    max_y: i32,
}

/// Which triangles survive culling.
#[derive(Clone, Copy, PartialEq)]
enum Cull {
    None,
    Back,
    Front,
}

struct Builder<'a> {
    view: &'a View3d,
    near: f32,
    flip: bool,
    tris: Vec<Tri>,
}

impl Builder<'_> {
    fn push(&mut self, corners: [Vert; 3], paint: u32, order: u32, cull: Cull, mirrored: bool) {
        let near = self.near;
        let orthographic = self.view.orthographic;
        let mut polygon: Vec<Vert> = corners.to_vec();
        if !orthographic && corners.iter().any(|v| v.view.z < near) {
            polygon = clip_near(&corners, near);
            if polygon.len() < 3 {
                return;
            }
        }
        let screen: Vec<[f32; 4]> = polygon
            .iter()
            .map(|v| {
                let [x, y] = self.view.view_to_output(v.view);
                let w = if orthographic { 1.0 } else { 1.0 / v.view.z };
                [x, y, v.view.z, w]
            })
            .collect();
        for i in 1..polygon.len() - 1 {
            let s = [screen[0], screen[i], screen[i + 1]];
            let area = (s[1][0] - s[0][0]) * (s[2][1] - s[0][1])
                - (s[1][1] - s[0][1]) * (s[2][0] - s[0][0]);
            if area.abs() < 1.0e-12 {
                continue;
            }
            // Front faces wind clockwise in y-down pixels.
            let front = (area < 0.0) != (self.flip ^ mirrored);
            match cull {
                Cull::Back if !front => continue,
                Cull::Front if front => continue,
                _ => {}
            }
            let min_y = s.iter().map(|p| p[1]).fold(f32::INFINITY, f32::min).floor() as i32;
            let max_y = s
                .iter()
                .map(|p| p[1])
                .fold(f32::NEG_INFINITY, f32::max)
                .ceil() as i32;
            self.tris.push(Tri {
                screen: s,
                verts: [polygon[0], polygon[i], polygon[i + 1]],
                paint,
                order,
                min_y,
                max_y,
            });
        }
    }
}

fn clip_near(corners: &[Vert; 3], near: f32) -> Vec<Vert> {
    let mut out = Vec::with_capacity(4);
    for i in 0..3 {
        let a = &corners[i];
        let b = &corners[(i + 1) % 3];
        let a_in = a.view.z >= near;
        let b_in = b.view.z >= near;
        if a_in {
            out.push(*a);
        }
        if a_in != b_in {
            let t = (near - a.view.z) / (b.view.z - a.view.z);
            out.push(lerp_vert(a, b, t));
        }
    }
    out
}

fn rgba_unit(color: crate::v2::Rgba) -> [f32; 4] {
    let a = f32::from(color.a) / 255.0;
    [
        f32::from(color.r) / 255.0 * a,
        f32::from(color.g) / 255.0 * a,
        f32::from(color.b) / 255.0 * a,
        a,
    ]
}

fn model_sampler(texture: &Texture) -> Sampler<'_> {
    Sampler {
        width: texture.width,
        height: texture.height,
        data: &texture.rgba,
        premultiplied: false,
        repeat: true,
        nearest: texture.pixelated,
    }
}

fn image_sampler(image: &Image) -> Sampler<'_> {
    Sampler {
        width: image.width,
        height: image.height,
        data: &image.rgba,
        premultiplied: true,
        repeat: false,
        nearest: false,
    }
}

fn render_once(frame: &Frame3d) -> Vec<u8> {
    let [w, h] = frame.view.output;
    let (near, _) = frame.depth_range();
    let to = frame.view.to_output;
    let mut builder = Builder {
        view: &frame.view,
        near: if frame.view.orthographic {
            f32::NEG_INFINITY
        } else {
            near
        },
        flip: to.a11 * to.a22 - to.a12 * to.a21 < 0.0,
        tris: Vec::new(),
    };
    let mut paints: Vec<Paint<'_>> = Vec::new();
    let passes = super::draw::draw_passes(&frame.items, &frame.view.view);
    for (index, item) in frame.items.iter().enumerate() {
        let (opaque_pass, blended_pass) = passes[index];
        let item_blends = item.opacity() < 0.999;
        match item {
            DrawItem::Mesh(mesh) => {
                let matrix = mesh.matrix;
                let normal_matrix = matrix
                    .inverse()
                    .map(|inverse| inverse.transpose())
                    .unwrap_or(matrix);
                let mirrored = matrix.determinant3() < 0.0;
                let render = &mesh.render;
                let positions = mesh
                    .posed
                    .as_ref()
                    .map(|p| p.positions.as_slice())
                    .unwrap_or(&render.positions);
                let normals = mesh
                    .posed
                    .as_ref()
                    .map(|p| p.normals.as_slice())
                    .unwrap_or(&render.normals);
                let verts: Vec<Vert> = (0..positions.len())
                    .map(|i| {
                        let world = matrix.transform_point(positions[i]);
                        let color = if render.has_colors {
                            let c = render.colors[i];
                            let a = f32::from(c[3]) / 255.0;
                            [
                                f32::from(c[0]) / 255.0 * a,
                                f32::from(c[1]) / 255.0 * a,
                                f32::from(c[2]) / 255.0 * a,
                                a,
                            ]
                        } else {
                            [1.0; 4]
                        };
                        Vert {
                            view: frame.view.view.transform_point(world),
                            world,
                            normal: normal_matrix.transform_vector(normals[i]).normalized(),
                            uv: render.uvs[i],
                            color,
                        }
                    })
                    .collect();
                for (material_index, first, count) in &render.parts {
                    let Some(material) = mesh.model.materials.get(usize::from(*material_index))
                    else {
                        continue;
                    };
                    let mut base = rgba_unit(material.color);
                    for c in &mut base {
                        *c *= mesh.opacity;
                    }
                    let blends = item_blends || material.blends(&mesh.model.textures);
                    let order = if blends { blended_pass } else { opaque_pass };
                    let paint = paints.len() as u32;
                    paints.push(Paint {
                        base,
                        texture: material
                            .texture
                            .and_then(|t| mesh.model.textures.get(usize::from(t)))
                            .map(model_sampler),
                        surface: Surface::of(material),
                        lit: mesh.lit && material.shading != Shading::Unlit,
                        cutoff: material.alpha_cutoff,
                        opacity: mesh.opacity,
                        vertex_colors: render.has_colors,
                        blends,
                        depth_test: true,
                    });
                    let cull = if material.double_sided {
                        Cull::None
                    } else {
                        Cull::Back
                    };
                    let indices = &render.indices[*first as usize..(*first + *count) as usize];
                    for t in indices.as_chunks::<3>().0.iter() {
                        builder.push(
                            [
                                verts[t[0] as usize],
                                verts[t[1] as usize],
                                verts[t[2] as usize],
                            ],
                            paint,
                            order,
                            cull,
                            mirrored,
                        );
                    }
                }
                if let (true, Some(outline)) = (mesh.outline, mesh.model.outline) {
                    if outline.width > 0.0 {
                        // The inverted hull: back faces pushed out along
                        // smoothed normals, a constant width on screen.
                        let mut smooth =
                            vec![V3::ZERO; mesh.model.meshes[mesh.mesh].positions.len()];
                        for (i, source) in render.source.iter().enumerate() {
                            if let Some(slot) = smooth.get_mut(*source as usize) {
                                *slot += verts[i].normal;
                            }
                        }
                        let inverse_view = frame.view.view.inverse().unwrap_or(frame.view.view);
                        let shell: Vec<Vert> = verts
                            .iter()
                            .enumerate()
                            .map(|(i, v)| {
                                let n = smooth
                                    .get(render.source[i] as usize)
                                    .map(|n| n.normalized())
                                    .unwrap_or(v.normal);
                                let reach =
                                    outline.width / frame.view.stage_scale_at(v.view.z).max(1.0e-6);
                                let world = v.world + n * reach;
                                let _ = inverse_view;
                                Vert {
                                    view: frame.view.view.transform_point(world),
                                    world,
                                    ..*v
                                }
                            })
                            .collect();
                        let mut base = rgba_unit(outline.color);
                        for c in &mut base {
                            *c *= mesh.opacity;
                        }
                        let order = if item_blends {
                            blended_pass
                        } else {
                            opaque_pass
                        };
                        let paint = paints.len() as u32;
                        paints.push(Paint {
                            base,
                            texture: None,
                            surface: Surface::MATTE,
                            lit: false,
                            cutoff: None,
                            opacity: mesh.opacity,
                            vertex_colors: false,
                            blends: item_blends,
                            depth_test: true,
                        });
                        for t in render.indices.as_chunks::<3>().0.iter() {
                            builder.push(
                                [
                                    shell[t[0] as usize],
                                    shell[t[1] as usize],
                                    shell[t[2] as usize],
                                ],
                                paint,
                                order,
                                Cull::Front,
                                mirrored,
                            );
                        }
                    }
                }
            }
            DrawItem::Plane(plane) => {
                let [x0, y0, x1, y1] = plane.rect;
                let normal = plane
                    .matrix
                    .inverse()
                    .map(|inverse| inverse.transpose())
                    .unwrap_or(plane.matrix)
                    .transform_vector(-V3::Z)
                    .normalized();
                let corner = |x: f32, y: f32, u: f32, v: f32| {
                    let world = plane.matrix.transform_point(V3::new(x, y, 0.0));
                    Vert {
                        view: frame.view.view.transform_point(world),
                        world,
                        normal,
                        uv: [u, v],
                        color: [1.0; 4],
                    }
                };
                let a = corner(x0, y0, 0.0, 0.0);
                let b = corner(x1, y0, 1.0, 0.0);
                let c = corner(x1, y1, 1.0, 1.0);
                let d = corner(x0, y1, 0.0, 1.0);
                let paint = paints.len() as u32;
                paints.push(Paint {
                    base: [plane.opacity; 4],
                    texture: Some(image_sampler(&plane.image)),
                    surface: Surface::MATTE,
                    lit: plane.lit && frame.rig.authored,
                    cutoff: None,
                    opacity: plane.opacity,
                    vertex_colors: false,
                    blends: true,
                    depth_test: !plane.layered,
                });
                builder.push([a, b, c], paint, blended_pass, Cull::None, false);
                builder.push([a, c, d], paint, blended_pass, Cull::None, false);
            }
        }
    }
    let mut tris = builder.tris;
    // Stable: triangles keep their drawing order within a pass.
    tris.sort_by_key(|tri| tri.order);
    let mut color = vec![0.0f32; (w * h * 4) as usize];
    if let Some([r, g, b, a]) = frame.clear {
        let a = f32::from(a) / 255.0;
        let fill = [
            f32::from(r) / 255.0 * a,
            f32::from(g) / 255.0 * a,
            f32::from(b) / 255.0 * a,
            a,
        ];
        for px in color.as_chunks_mut::<4>().0.iter_mut() {
            px.copy_from_slice(&fill);
        }
    }
    let mut depth = vec![f32::INFINITY; (w * h) as usize];
    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .clamp(1, 16);
    let rows_per_band = ((h as usize).div_ceil(threads * 2)).max(8);
    let shadows: Vec<ShadowMap> = shadow_views(frame)
        .into_iter()
        .map(|view| ShadowMap::build(frame, view))
        .collect();
    let ctx = Context {
        frame,
        paints: &paints,
        tris: &tris,
        width: w as usize,
        shadows: &shadows,
    };
    std::thread::scope(|scope| {
        let mut color_rest: &mut [f32] = &mut color;
        let mut depth_rest: &mut [f32] = &mut depth;
        let mut y0 = 0usize;
        let ctx = &ctx;
        while y0 < h as usize {
            let rows = rows_per_band.min(h as usize - y0);
            let (color_band, c_rest) = color_rest.split_at_mut(rows * w as usize * 4);
            let (depth_band, d_rest) = depth_rest.split_at_mut(rows * w as usize);
            color_rest = c_rest;
            depth_rest = d_rest;
            let band_y = y0;
            scope.spawn(move || ctx.band(band_y, rows, color_band, depth_band));
            y0 += rows;
        }
    });
    color
        .iter()
        .map(|c| (c.clamp(0.0, 1.0) * 255.0 + 0.5) as u8)
        .collect()
}

struct Context<'a> {
    frame: &'a Frame3d,
    paints: &'a [Paint<'a>],
    tris: &'a [Tri],
    width: usize,
    shadows: &'a [ShadowMap],
}

/// Texels across a software shadow map.
const SOFT_SHADOW: usize = 1024;

/// Depth seen from a light, nearest surface per texel, 0..1.
struct ShadowMap {
    view: ShadowView,
    depth: Vec<f32>,
    /// World units per texel here.
    texel: f32,
}

impl ShadowMap {
    fn build(frame: &Frame3d, view: ShadowView) -> Self {
        let size = SOFT_SHADOW;
        let mut depth = vec![1.0f32; size * size];
        let m = view.matrix;
        let to_map = |p: V3| -> Option<[f32; 3]> {
            let c = m.transform4(p);
            if c[3] <= 1.0e-6 {
                return None;
            }
            Some([
                (c[0] / c[3] * 0.5 + 0.5) * size as f32,
                (c[1] / c[3] * 0.5 + 0.5) * size as f32,
                c[2] / c[3] * 0.5 + 0.5,
            ])
        };
        let mut raster =
            |a: [f32; 3], b: [f32; 3], c: [f32; 3], cut: &dyn Fn(f32, f32, f32) -> bool| {
                let area = (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
                if area.abs() < 1.0e-9 {
                    return;
                }
                let x0 = a[0].min(b[0]).min(c[0]).floor().max(0.0) as usize;
                let x1 = (a[0].max(b[0]).max(c[0]).ceil() as isize).min(size as isize - 1);
                let y0 = a[1].min(b[1]).min(c[1]).floor().max(0.0) as usize;
                let y1 = (a[1].max(b[1]).max(c[1]).ceil() as isize).min(size as isize - 1);
                if x1 < 0 || y1 < 0 {
                    return;
                }
                for y in y0..=y1 as usize {
                    for x in x0..=x1 as usize {
                        let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                        let w0 = ((b[0] - px) * (c[1] - py) - (b[1] - py) * (c[0] - px)) / area;
                        let w1 = ((c[0] - px) * (a[1] - py) - (c[1] - py) * (a[0] - px)) / area;
                        let w2 = 1.0 - w0 - w1;
                        if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                            continue;
                        }
                        if cut(w0, w1, w2) {
                            continue;
                        }
                        let d = w0 * a[2] + w1 * b[2] + w2 * c[2];
                        let slot = &mut depth[y * size + x];
                        if d < *slot {
                            *slot = d;
                        }
                    }
                }
            };
        for item in &frame.items {
            match item {
                DrawItem::Mesh(mesh) => {
                    if mesh.opacity < 0.5 {
                        continue;
                    }
                    let positions = mesh
                        .posed
                        .as_ref()
                        .map(|p| p.positions.as_slice())
                        .unwrap_or(&mesh.render.positions);
                    let points: Vec<Option<[f32; 3]>> = positions
                        .iter()
                        .map(|p| to_map(mesh.matrix.transform_point(*p)))
                        .collect();
                    for t in mesh.render.indices.as_chunks::<3>().0.iter() {
                        if let (Some(a), Some(b), Some(c)) = (
                            points[t[0] as usize],
                            points[t[1] as usize],
                            points[t[2] as usize],
                        ) {
                            raster(a, b, c, &|_, _, _| false);
                        }
                    }
                }
                DrawItem::Plane(plane) => {
                    if plane.layered || plane.opacity < 0.5 {
                        continue;
                    }
                    let [x0, y0, x1, y1] = plane.rect;
                    let corner = |x, y| to_map(plane.matrix.transform_point(V3::new(x, y, 0.0)));
                    let sampler = image_sampler(&plane.image);
                    let (Some(a), Some(b), Some(c), Some(d)) = (
                        corner(x0, y0),
                        corner(x1, y0),
                        corner(x1, y1),
                        corner(x0, y1),
                    ) else {
                        continue;
                    };
                    // Cut-out cards cast their shape, not their rectangle.
                    raster(a, b, c, &|_, w1, w2| sampler.sample([w1 + w2, w2])[3] < 0.5);
                    raster(a, c, d, &|_, w1, w2| sampler.sample([w1, w1 + w2])[3] < 0.5);
                }
            }
        }
        let texel = view.texel * (SHADOW_RATIO);
        Self { view, depth, texel }
    }

    /// How much of the light reaches `world` (with surface normal `n`).
    fn visible(&self, world: V3, n: V3) -> f32 {
        let size = SOFT_SHADOW;
        // Pushed off the surface a little so it does not shade itself.
        let p = world + n * (self.texel * 1.5);
        let c = self.view.matrix.transform4(p);
        if c[3] <= 1.0e-6 {
            return 1.0;
        }
        let (x, y, z) = (c[0] / c[3], c[1] / c[3], c[2] / c[3] * 0.5 + 0.5);
        if !(-1.0..=1.0).contains(&x) || !(-1.0..=1.0).contains(&y) || z > 1.0 {
            return 1.0;
        }
        let fx = (x * 0.5 + 0.5) * size as f32 - 0.5;
        let fy = (y * 0.5 + 0.5) * size as f32 - 0.5;
        let mut lit = 0.0;
        for dy in -1..=1 {
            for dx in -1..=1 {
                let sx = (fx.round() as isize + dx).clamp(0, size as isize - 1) as usize;
                let sy = (fy.round() as isize + dy).clamp(0, size as isize - 1) as usize;
                if z - 0.0005 <= self.depth[sy * size + sx] {
                    lit += 1.0;
                }
            }
        }
        lit / 9.0
    }
}

/// The software map is coarser than the GPU's.
const SHADOW_RATIO: f32 = (super::draw::SHADOW_SIZE / SOFT_SHADOW) as f32;

impl Context<'_> {
    fn band(&self, y0: usize, rows: usize, color: &mut [f32], depth: &mut [f32]) {
        let (band_top, band_bottom) = (y0 as i32, (y0 + rows) as i32);
        let width = self.width as i32;
        for tri in self.tris {
            if tri.max_y < band_top || tri.min_y >= band_bottom {
                continue;
            }
            let paint = &self.paints[tri.paint as usize];
            let [p0, p1, p2] = tri.screen;
            let min_x = p0[0].min(p1[0]).min(p2[0]).floor().max(0.0) as i32;
            let max_x = (p0[0].max(p1[0]).max(p2[0]).ceil() as i32).min(width - 1);
            let min_y = tri.min_y.max(band_top);
            let max_y = tri.max_y.min(band_bottom - 1);
            if min_x > max_x || min_y > max_y {
                continue;
            }
            let area = (p1[0] - p0[0]) * (p2[1] - p0[1]) - (p1[1] - p0[1]) * (p2[0] - p0[0]);
            let inv_area = 1.0 / area;
            for y in min_y..=max_y {
                let py = y as f32 + 0.5;
                let row = (y - band_top) as usize * self.width;
                for x in min_x..=max_x {
                    let px = x as f32 + 0.5;
                    let w0 = ((p1[0] - px) * (p2[1] - py) - (p1[1] - py) * (p2[0] - px)) * inv_area;
                    let w1 = ((p2[0] - px) * (p0[1] - py) - (p2[1] - py) * (p0[0] - px)) * inv_area;
                    let w2 = 1.0 - w0 - w1;
                    if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                        continue;
                    }
                    let index = row + x as usize;
                    // Perspective-correct weights.
                    let q0 = w0 * p0[3];
                    let q1 = w1 * p1[3];
                    let q2 = w2 * p2[3];
                    let sum = q0 + q1 + q2;
                    if sum <= 0.0 {
                        continue;
                    }
                    let (b0, b1, b2) = (q0 / sum, q1 / sum, q2 / sum);
                    let z = b0 * p0[2] + b1 * p1[2] + b2 * p2[2];
                    if paint.depth_test && z >= depth[index] {
                        continue;
                    }
                    let Some(rgba) = self.shade_pixel(tri, paint, b0, b1, b2) else {
                        continue;
                    };
                    let out = &mut color[index * 4..index * 4 + 4];
                    if paint.blends {
                        let keep = 1.0 - rgba[3];
                        for c in 0..4 {
                            out[c] = rgba[c] + out[c] * keep;
                        }
                        if paint.depth_test && rgba[3] >= 0.5 {
                            depth[index] = z;
                        }
                    } else {
                        out.copy_from_slice(&rgba);
                        depth[index] = z;
                    }
                }
            }
        }
    }

    fn shade_pixel(
        &self,
        tri: &Tri,
        paint: &Paint<'_>,
        b0: f32,
        b1: f32,
        b2: f32,
    ) -> Option<[f32; 4]> {
        let [v0, v1, v2] = &tri.verts;
        let uv = [
            v0.uv[0] * b0 + v1.uv[0] * b1 + v2.uv[0] * b2,
            v0.uv[1] * b0 + v1.uv[1] * b1 + v2.uv[1] * b2,
        ];
        let mut rgba = paint.base;
        if let Some(texture) = &paint.texture {
            let texel = texture.sample(uv);
            for c in 0..4 {
                rgba[c] *= texel[c];
            }
        }
        if paint.vertex_colors {
            for (c, channel) in rgba.iter_mut().enumerate() {
                *channel *= v0.color[c] * b0 + v1.color[c] * b1 + v2.color[c] * b2;
            }
        }
        if let Some(cutoff) = paint.cutoff {
            let alpha = rgba[3] / paint.opacity.max(1.0e-6);
            if alpha < cutoff {
                return None;
            }
            // Cut-out pixels are fully there.
            let k = paint.opacity / rgba[3].max(1.0e-6);
            for c in &mut rgba {
                *c *= k;
            }
        }
        if rgba[3] <= 1.0 / 512.0 {
            return None;
        }
        let world = v0.world * b0 + v1.world * b1 + v2.world * b2;
        if paint.lit {
            let mut n = (v0.normal * b0 + v1.normal * b1 + v2.normal * b2).normalized();
            let to_eye = self.frame.view.toward_eye(world);
            if n.dot(to_eye) < 0.0 {
                n = -n;
            }
            if paint.surface.shading == Shading::Glass {
                return Some(self.hazed(
                    super::draw::shade_glass(&self.frame.rig, rgba, world, n, to_eye),
                    world,
                ));
            }
            let (gain, shine) =
                shade_with(&self.frame.rig, &paint.surface, world, n, to_eye, |light| {
                    self.shadows
                        .iter()
                        .find(|map| map.view.light == light)
                        .map_or(1.0, |map| map.visible(world, n))
                });
            for c in 0..3 {
                rgba[c] = (rgba[c] * gain[c] + shine[c] * rgba[3]).min(rgba[3]);
            }
        }
        Some(self.hazed(rgba, world))
    }

    /// Fades a premultiplied pixel at `world` into the haze.
    fn hazed(&self, mut rgba: [f32; 4], world: V3) -> [f32; 4] {
        let view = &self.frame.view;
        let distance = if view.orthographic {
            (world - view.camera.position)
                .dot((view.camera.target - view.camera.position).normalized())
        } else {
            (world - view.camera.position).length()
        };
        if let Some((color, amount)) = self.frame.rig.fog_at(distance) {
            for c in 0..3 {
                rgba[c] += (color[c] * rgba[3] - rgba[c]) * amount;
            }
        }
        rgba
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::three::draw::{push_model, View3d};
    use crate::three::math::{Mat4, Quat};
    use crate::three::model::{Material, Mesh, ModelData, Node};
    use crate::three::scene::{CameraPose, LightRig};
    use crate::v2::Rgba;
    use std::sync::Arc;

    /// A unit cube with outward, counter-clockwise faces.
    pub(crate) fn cube() -> Mesh {
        let mut mesh = Mesh {
            name: "cube".into(),
            ..Default::default()
        };
        for z in [-1.0, 1.0] {
            for y in [-1.0, 1.0] {
                for x in [-1.0, 1.0] {
                    mesh.positions.push(V3::new(x, y, z));
                }
            }
        }
        let faces: [[u32; 4]; 6] = [
            [4, 5, 7, 6], // +z
            [1, 0, 2, 3], // -z
            [5, 1, 3, 7], // +x
            [0, 4, 6, 2], // -x
            [2, 6, 7, 3], // +y
            [0, 1, 5, 4], // -y
        ];
        for face in faces {
            mesh.push_face(&face, None, 0, false);
        }
        mesh
    }

    fn frame_of(model: ModelData, placement: Mat4) -> Frame3d {
        let model = Arc::new(model);
        let camera = CameraPose::stage(200, 200);
        let view = View3d::new(camera, false, [200.0, 200.0], Affine::IDENTITY, [200, 200]);
        let mut items = Vec::new();
        push_model(
            &mut items,
            &model,
            &placement,
            &model.rest_world(),
            1.0,
            true,
        );
        Frame3d {
            rig: LightRig::default_for(&camera),
            view,
            items,
            overlay: None,
            clear: None,
        }
    }

    fn red_cube() -> ModelData {
        let mut model = ModelData::default();
        model.materials[0] = Material::with_color(
            "red",
            Rgba {
                r: 220,
                g: 30,
                b: 30,
                a: 255,
            },
        );
        model.scale = 40.0;
        model.nodes.push(Node::new("cube"));
        let mut mesh = cube();
        mesh.node = Some(0);
        model.meshes.push(mesh);
        model
    }

    #[test]
    fn a_cube_in_front_of_the_camera_shows_its_faces() {
        let placement = Mat4::translation(V3::new(100.0, 100.0, 0.0))
            * Mat4::from_quat(Quat::from_euler_deg(25.0, 35.0, 0.0));
        let pixels = render(&frame_of(red_cube(), placement), 1);
        let at = |x: usize, y: usize| &pixels[(y * 200 + x) * 4..(y * 200 + x) * 4 + 4];
        assert_eq!(at(100, 100)[3], 255, "the middle is covered");
        assert!(at(100, 100)[0] > at(100, 100)[1], "and red");
        assert_eq!(at(5, 5)[3], 0, "the corner is empty");
        // Two faces seen at different angles get different light.
        let lit: std::collections::BTreeSet<u8> = (60..140)
            .step_by(8)
            .map(|x| at(x, 100)[0])
            .filter(|r| *r > 0)
            .collect();
        assert!(lit.len() >= 2, "{lit:?}");
    }

    #[test]
    fn a_cube_partly_behind_the_camera_is_cut_not_lost() {
        // Just below the camera and reaching behind it: its top is cut at
        // the near plane, the rest still shows.
        let camera = CameraPose::stage(200, 200);
        let placement = Mat4::translation(V3::new(100.0, 150.0, camera.position.z + 30.0));
        let pixels = render(&frame_of(red_cube(), placement), 1);
        let covered = pixels
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|px| px[3] > 0)
            .count();
        assert!(covered > 5_000, "{covered}");
    }

    #[test]
    fn supersampling_softens_edges() {
        let placement = Mat4::translation(V3::new(100.0, 100.0, 0.0))
            * Mat4::from_quat(Quat::from_euler_deg(0.0, 0.0, 20.0));
        let pixels = render(&frame_of(red_cube(), placement), 2);
        assert!(pixels
            .as_chunks::<4>()
            .0
            .iter()
            .any(|px| px[3] > 0 && px[3] < 255));
    }
}
