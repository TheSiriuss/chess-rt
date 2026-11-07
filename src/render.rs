//! OpenGL underlay renderer: scene shader -> HDR texture (with mips) -> post shader -> window.
//! Runs inside Slint's BeforeRendering notifier, so all GL state we touch is restored.

use glow::HasContext;

pub struct Uniforms {
    pub board: [f32; 3], // left, top, size in physical px
    pub time: f32,
    pub pieces: [i32; 64],
    pub marks: [i32; 64],
    pub movers: [[f32; 4]; 4],
    pub mover_scale: [f32; 4],
    pub opt: [f32; 4],
    pub glow0: [f32; 4],
    pub glow0_col: [f32; 3],
    pub glow1: [f32; 4],
    pub glow1_col: [f32; 3],
    pub top_z: f32,
    pub bloom: f32,
    pub exposure: f32,
    pub scale: f32, // render resolution scale (0.5..1)
    pub sun: [f32; 4],
    pub sun_col: [f32; 3],
    pub night: f32,
    pub rain: f32,
    pub flash: f32,
    pub dusk: f32,
    pub pl: [[f32; 4]; 8],
    pub pl_col: [[f32; 4]; 8],
    pub n_pl: i32,
    pub n_lantern: i32,
    pub fish: [[f32; 4]; 6],
    pub fish_k: [[f32; 4]; 6],
    pub n_fish: i32,
    pub rip: [[f32; 4]; 10],
    pub food: [[f32; 4]; 6],
    pub sunk: [i32; 32],
    pub sink_slot: i32,
    pub sink_t: f32,
}

struct Target {
    fbo: glow::Framebuffer,
    tex: glow::Texture,
    w: i32,
    h: i32,
    levels: i32,
}

pub struct Renderer {
    gl: glow::Context,
    scene: glow::Program,
    post: glow::Program,
    vao: glow::VertexArray,
    target: Option<Target>,
    /// baked static albedo layer (wood, pebbles) and the layout it was baked for
    static_t: Option<Target>,
    static_key: [f32; 5],
    hdr_format: (u32, u32, u32), // internal, format, type
    pub info: String,
}

fn compile(gl: &glow::Context, header: &str, vs: &str, fs: &str) -> Result<glow::Program, String> {
    unsafe {
        let prog = gl.create_program()?;
        let mut shaders = vec![];
        for (ty, src) in [(glow::VERTEX_SHADER, vs), (glow::FRAGMENT_SHADER, fs)] {
            let sh = gl.create_shader(ty)?;
            gl.shader_source(sh, &format!("{header}{src}"));
            gl.compile_shader(sh);
            if !gl.get_shader_compile_status(sh) {
                return Err(format!("shader compile error:\n{}", gl.get_shader_info_log(sh)));
            }
            gl.attach_shader(prog, sh);
            shaders.push(sh);
        }
        gl.link_program(prog);
        if !gl.get_program_link_status(prog) {
            return Err(format!("link error: {}", gl.get_program_info_log(prog)));
        }
        for sh in shaders {
            gl.detach_shader(prog, sh);
            gl.delete_shader(sh);
        }
        Ok(prog)
    }
}

impl Renderer {
    pub fn new(gl: glow::Context) -> Result<Renderer, String> {
        let ver = gl.version().clone();
        let header = if ver.is_embedded {
            "#version 300 es\nprecision highp float;\nprecision highp int;\n"
        } else {
            "#version 330 core\n"
        };
        let vs = include_str!("../shaders/fullscreen.vert");
        let scene = compile(&gl, header, vs, include_str!("../shaders/scene.frag"))?;
        let post = compile(&gl, header, vs, include_str!("../shaders/post.frag"))?;
        let vao = unsafe { gl.create_vertex_array()? };
        let info = unsafe {
            format!(
                "{} · {}",
                gl.get_parameter_string(glow::RENDERER),
                gl.get_parameter_string(glow::VERSION)
            )
        };
        Ok(Renderer { gl, scene, post, vao, target: None, static_t: None, static_key: [0.0; 5], hdr_format: (glow::RGBA16F, glow::RGBA, glow::HALF_FLOAT), info })
    }

    fn ensure_target(&mut self, w: i32, h: i32) {
        if let Some(t) = &self.target {
            if t.w == w && t.h == h {
                return;
            }
        }
        let gl = &self.gl;
        unsafe {
            if let Some(t) = self.target.take() {
                gl.delete_framebuffer(t.fbo);
                gl.delete_texture(t.tex);
            }
            let levels = (32 - (w.max(h) as u32).leading_zeros()) as i32;
            for fmt in [(glow::RGBA16F, glow::RGBA, glow::HALF_FLOAT), (glow::RGBA8, glow::RGBA, glow::UNSIGNED_BYTE)] {
                let tex = gl.create_texture().unwrap();
                gl.bind_texture(glow::TEXTURE_2D, Some(tex));
                gl.tex_storage_2d(glow::TEXTURE_2D, levels, fmt.0, w, h);
                gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::LINEAR_MIPMAP_LINEAR as i32);
                gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::LINEAR as i32);
                gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE as i32);
                gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE as i32);
                let fbo = gl.create_framebuffer().unwrap();
                gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
                gl.framebuffer_texture_2d(glow::FRAMEBUFFER, glow::COLOR_ATTACHMENT0, glow::TEXTURE_2D, Some(tex), 0);
                if gl.check_framebuffer_status(glow::FRAMEBUFFER) == glow::FRAMEBUFFER_COMPLETE {
                    self.hdr_format = fmt;
                    self.target = Some(Target { fbo, tex, w, h, levels });
                    break;
                }
                eprintln!("render target format {:#x} not supported, falling back", fmt.0);
                gl.delete_framebuffer(fbo);
                gl.delete_texture(tex);
            }
        }
    }

    fn ensure_static(&mut self, w: i32, h: i32) {
        if let Some(t) = &self.static_t {
            if t.w == w && t.h == h {
                return;
            }
        }
        let gl = &self.gl;
        unsafe {
            if let Some(t) = self.static_t.take() {
                gl.delete_framebuffer(t.fbo);
                gl.delete_texture(t.tex);
            }
            let tex = gl.create_texture().unwrap();
            gl.bind_texture(glow::TEXTURE_2D, Some(tex));
            gl.tex_storage_2d(glow::TEXTURE_2D, 1, glow::RGBA8, w, h);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::LINEAR as i32);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::LINEAR as i32);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE as i32);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE as i32);
            let fbo = gl.create_framebuffer().unwrap();
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
            gl.framebuffer_texture_2d(glow::FRAMEBUFFER, glow::COLOR_ATTACHMENT0, glow::TEXTURE_2D, Some(tex), 0);
            self.static_t = Some(Target { fbo, tex, w, h, levels: 1 });
            self.static_key = [0.0; 5]; // force a bake
        }
    }

    pub fn hdr(&self) -> bool {
        self.hdr_format.0 == glow::RGBA16F
    }

    /// Draw the full frame into the currently bound framebuffer of size (win_w, win_h).
    pub fn render(&mut self, win_w: u32, win_h: u32, u: &Uniforms) {
        let scale = u.scale.clamp(0.25, 1.0);
        let w = ((win_w as f32 * scale) as i32).max(8);
        let h = ((win_h as f32 * scale) as i32).max(8);
        unsafe {
            let gl = &self.gl;
            // ---- save state
            let prev_fbo = gl.get_parameter_i32(glow::FRAMEBUFFER_BINDING);
            let prev_prog = gl.get_parameter_i32(glow::CURRENT_PROGRAM);
            let prev_vao = gl.get_parameter_i32(glow::VERTEX_ARRAY_BINDING);
            let prev_active = gl.get_parameter_i32(glow::ACTIVE_TEXTURE);
            gl.active_texture(glow::TEXTURE0);
            let prev_tex = gl.get_parameter_i32(glow::TEXTURE_BINDING_2D);
            let mut vp = [0i32; 4];
            gl.get_parameter_i32_slice(glow::VIEWPORT, &mut vp);
            let caps = [glow::BLEND, glow::SCISSOR_TEST, glow::STENCIL_TEST, glow::DEPTH_TEST, glow::CULL_FACE];
            let caps_on: Vec<bool> = caps.iter().map(|&c| gl.is_enabled(c)).collect();
            for &c in &caps {
                gl.disable(c);
            }
            let prev_fbo_obj = std::num::NonZeroU32::new(prev_fbo as u32).map(glow::NativeFramebuffer);

            self.ensure_target(w, h);
            self.ensure_static(w, h);
            let gl = &self.gl;
            gl.bind_vertex_array(Some(self.vao));
            gl.active_texture(glow::TEXTURE1);
            let prev_tex1 = gl.get_parameter_i32(glow::TEXTURE_BINDING_2D);
            gl.active_texture(glow::TEXTURE0);

            // ---- bake the static layer when the layout changed
            let key = [w as f32, h as f32, u.board[0] * scale, u.board[1] * scale, u.board[2] * scale];
            let st = self.static_t.as_ref().unwrap();
            if key != self.static_key {
                gl.bind_framebuffer(glow::FRAMEBUFFER, Some(st.fbo));
                gl.viewport(0, 0, w, h);
                gl.use_program(Some(self.scene));
                let p = self.scene;
                gl.uniform_1_i32(gl.get_uniform_location(p, "uPass").as_ref(), 1);
                gl.uniform_2_f32(gl.get_uniform_location(p, "uRes").as_ref(), w as f32, h as f32);
                gl.uniform_3_f32(gl.get_uniform_location(p, "uBoard").as_ref(), key[2], key[3], key[4]);
                gl.draw_arrays(glow::TRIANGLES, 0, 3);
                self.static_key = key;
            }
            let static_tex = st.tex;
            let t = self.target.as_ref().unwrap();

            // ---- scene pass
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(t.fbo));
            gl.viewport(0, 0, t.w, t.h);
            gl.use_program(Some(self.scene));
            let p = self.scene;
            let loc = |n: &str| gl.get_uniform_location(p, n);
            gl.uniform_1_i32(loc("uPass").as_ref(), 0);
            gl.active_texture(glow::TEXTURE1);
            gl.bind_texture(glow::TEXTURE_2D, Some(static_tex));
            gl.active_texture(glow::TEXTURE0);
            gl.uniform_1_i32(loc("uStatic").as_ref(), 1);
            gl.uniform_2_f32(loc("uRes").as_ref(), t.w as f32, t.h as f32);
            gl.uniform_3_f32(loc("uBoard").as_ref(), u.board[0] * scale, u.board[1] * scale, u.board[2] * scale);
            gl.uniform_1_f32(loc("uTime").as_ref(), u.time);
            gl.uniform_1_i32_slice(loc("uPieces").as_ref(), &u.pieces);
            gl.uniform_1_i32_slice(loc("uMarks").as_ref(), &u.marks);
            let movers: Vec<f32> = u.movers.iter().flatten().copied().collect();
            gl.uniform_4_f32_slice(loc("uMov").as_ref(), &movers);
            gl.uniform_1_f32_slice(loc("uMovScale").as_ref(), &u.mover_scale);
            gl.uniform_4_f32_slice(loc("uOpt").as_ref(), &u.opt);
            gl.uniform_4_f32_slice(loc("uGlow0").as_ref(), &u.glow0);
            gl.uniform_3_f32_slice(loc("uGlow0Col").as_ref(), &u.glow0_col);
            gl.uniform_4_f32_slice(loc("uGlow1").as_ref(), &u.glow1);
            gl.uniform_3_f32_slice(loc("uGlow1Col").as_ref(), &u.glow1_col);
            gl.uniform_1_f32(loc("uTopZ").as_ref(), u.top_z);
            gl.uniform_4_f32_slice(loc("uSun").as_ref(), &u.sun);
            gl.uniform_3_f32_slice(loc("uSunCol").as_ref(), &u.sun_col);
            gl.uniform_1_f32(loc("uNight").as_ref(), u.night);
            gl.uniform_1_f32(loc("uRain").as_ref(), u.rain);
            gl.uniform_1_f32(loc("uFlash").as_ref(), u.flash);
            gl.uniform_1_f32(loc("uDusk").as_ref(), u.dusk);
            let flat = |v: &[[f32; 4]]| v.iter().flatten().copied().collect::<Vec<f32>>();
            gl.uniform_4_f32_slice(loc("uPL").as_ref(), &flat(&u.pl));
            gl.uniform_4_f32_slice(loc("uPLCol").as_ref(), &flat(&u.pl_col));
            gl.uniform_1_i32(loc("uNPL").as_ref(), u.n_pl);
            gl.uniform_1_i32(loc("uNLantern").as_ref(), u.n_lantern);
            gl.uniform_4_f32_slice(loc("uFish").as_ref(), &flat(&u.fish));
            gl.uniform_4_f32_slice(loc("uFishK").as_ref(), &flat(&u.fish_k));
            gl.uniform_1_i32(loc("uNFish").as_ref(), u.n_fish);
            gl.uniform_4_f32_slice(loc("uRip").as_ref(), &flat(&u.rip));
            gl.uniform_4_f32_slice(loc("uFood").as_ref(), &flat(&u.food));
            gl.uniform_1_i32_slice(loc("uSunk").as_ref(), &u.sunk);
            gl.uniform_1_i32(loc("uSinkSlot").as_ref(), u.sink_slot);
            gl.uniform_1_f32(loc("uSinkT").as_ref(), u.sink_t);
            gl.draw_arrays(glow::TRIANGLES, 0, 3);

            // ---- mips for bloom
            gl.bind_texture(glow::TEXTURE_2D, Some(t.tex));
            let bloom = if u.bloom > 0.0 && t.levels > 1 {
                gl.generate_mipmap(glow::TEXTURE_2D);
                u.bloom
            } else {
                0.0
            };

            // ---- post pass to the window
            gl.bind_framebuffer(glow::FRAMEBUFFER, prev_fbo_obj);
            gl.viewport(0, 0, win_w as i32, win_h as i32);
            gl.use_program(Some(self.post));
            let p = self.post;
            let loc = |n: &str| gl.get_uniform_location(p, n);
            gl.uniform_1_i32(loc("uTex").as_ref(), 0);
            gl.uniform_2_f32(loc("uOutRes").as_ref(), win_w as f32, win_h as f32);
            gl.uniform_1_f32(loc("uBloom").as_ref(), bloom);
            gl.uniform_1_f32(loc("uExposure").as_ref(), u.exposure);
            gl.draw_arrays(glow::TRIANGLES, 0, 3);

            // ---- restore state
            gl.active_texture(glow::TEXTURE1);
            gl.bind_texture(glow::TEXTURE_2D, std::num::NonZeroU32::new(prev_tex1 as u32).map(glow::NativeTexture));
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, std::num::NonZeroU32::new(prev_tex as u32).map(glow::NativeTexture));
            gl.active_texture(prev_active as u32);
            gl.use_program(std::num::NonZeroU32::new(prev_prog as u32).map(glow::NativeProgram));
            gl.bind_vertex_array(std::num::NonZeroU32::new(prev_vao as u32).map(glow::NativeVertexArray));
            gl.viewport(vp[0], vp[1], vp[2], vp[3]);
            for (i, &c) in caps.iter().enumerate() {
                if caps_on[i] {
                    gl.enable(c);
                }
            }
        }
    }

    /// Read back the current framebuffer as RGBA rows (top row first).
    pub fn read_pixels(&self, w: u32, h: u32) -> Vec<u8> {
        let mut buf = vec![0u8; (w * h * 4) as usize];
        unsafe {
            self.gl.read_pixels(
                0,
                0,
                w as i32,
                h as i32,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelPackData::Slice(Some(&mut buf)),
            );
        }
        let row = (w * 4) as usize;
        let mut out = Vec::with_capacity(buf.len());
        for y in (0..h as usize).rev() {
            out.extend_from_slice(&buf[y * row..(y + 1) * row]);
        }
        out
    }

    pub fn finish(&self) {
        unsafe { self.gl.finish() }
    }
}
