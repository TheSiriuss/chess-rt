// Scene: a chess board on a wooden raft in a garden pond, seen from above.
// Board & pieces: SDF silhouettes extruded into bevelled relief, soft ray-traced
// shadows from the sun/moon and from every moving light (lanterns, fireflies,
// lightning). Water: refraction, absorption, animated caustics, ripples, rain,
// koi with shadows on the pebbles, lily pads, captured pieces lying on the bottom.
//
// Board space: units = squares, origin = bottom-left corner of the 8x8 field,
// y up, z = height above the board surface.

out vec4 fragColor;

uniform vec2  uRes;        // render target size, px
uniform vec3  uBoard;      // board field: left px, top px (y down), size px (8 squares)
uniform float uTime;
uniform int   uPieces[64]; // screen-ordered cells (row 0 = bottom); 1..6 white PNBRQK, 7..12 black
uniform int   uMarks[64];  // bit flags, see MK_*
uniform vec4  uMov[4];     // movers: xy centre, z lift, w type (3 = captured piece flying into the pond)
uniform float uMovScale[4];
uniform vec4  uOpt;        // x shadows on, y softness k, z AO on, w specular on
uniform vec4  uGlow0;      // selection light: xyz, intensity
uniform vec3  uGlow0Col;
uniform vec4  uGlow1;      // check light
uniform vec3  uGlow1Col;
uniform float uTopZ;       // highest occluder top (for shadow ray early-out)
uniform vec4  uSun;        // xyz direction to the sun / moon, w intensity
uniform vec3  uSunCol;
uniform float uNight;      // 0 day .. 1 night
uniform float uRain;       // 0 .. 1
uniform float uFlash;      // lightning 0 .. 1
uniform float uDusk;       // sunrise / sunset glow 0 .. 1
uniform vec4  uPL[8];      // point lights: xyz, intensity (first uNLantern are floating lanterns, then fireflies)
uniform vec4  uPLCol[8];   // rgb, range
uniform int   uNPL;
uniform int   uNLantern;
uniform vec4  uFish[6];    // x, y, heading, swim phase
uniform vec4  uFishK[6];   // kind, size
uniform int   uNFish;
uniform vec4  uRip[10];    // ripples: x, y, start time, amplitude
uniform vec4  uFood[6];    // food pellets: x, y, alive, drop time
uniform int   uSunk[32];   // captured pieces on the pond bottom, 16 slots per side
uniform int   uSinkSlot;   // slot of the piece that is still sinking (-1 none)
uniform float uSinkT;      // its progress 0..1
uniform int   uPass;       // 1 = bake the static albedo layer (wood, pebbles) into uStatic
uniform sampler2D uStatic; // that baked layer, same size as the render target

const int MK_SEL = 1, MK_MOVE = 2, MK_CAPT = 4, MK_LAST = 8, MK_CHECK = 16, MK_HOVER = 32;

const float PH = 0.20;      // piece relief height
const float BEVEL = 0.06;
const float FRAME = 0.62;   // frame width around the field
const float PI = 3.14159265;
const float WZ = -0.25;     // water surface height
const vec3 ABSORB = vec3(0.85, 0.30, 0.50);   // water absorption per unit length (red goes first)

float pxSize;               // one pixel in board units
vec3 gAmb;                  // sky ambient

// ------------------------------------------------------------- noise
float hash12(vec2 p) {
    vec3 p3 = fract(vec3(p.xyx) * 0.1031);
    p3 += dot(p3, p3.yzx + 33.33);
    return fract((p3.x + p3.y) * p3.z);
}
float vnoise(vec2 p) {
    vec2 i = floor(p), f = fract(p);
    vec2 u = f * f * (3.0 - 2.0 * f);
    return mix(mix(hash12(i), hash12(i + vec2(1, 0)), u.x),
               mix(hash12(i + vec2(0, 1)), hash12(i + vec2(1, 1)), u.x), u.y);
}
float fbm(vec2 p) {
    float s = 0.0, a = 0.5;
    for (int i = 0; i < 5; i++) { s += a * vnoise(p); p = p * 2.03 + vec2(17.1, 3.7); a *= 0.5; }
    return s;
}

// ------------------------------------------------------------- 2D SDF primitives
float sdBox(vec2 p, vec2 b) { vec2 d = abs(p) - b; return length(max(d, 0.0)) + min(max(d.x, d.y), 0.0); }
float sdRBox(vec2 p, vec2 b, float r) { return sdBox(p, b - r) - r; }
float smin(float a, float b, float k) { float h = clamp(0.5 + 0.5 * (b - a) / k, 0.0, 1.0); return mix(b, a, h) - k * h * (1.0 - h); }
// trapezoid: r1 = half width at bottom, r2 = half width at top, he = half height
float sdTrap(vec2 p, float r1, float r2, float he) {
    vec2 k1 = vec2(r2, he);
    vec2 k2 = vec2(r2 - r1, 2.0 * he);
    p.x = abs(p.x);
    vec2 ca = vec2(p.x - min(p.x, (p.y < 0.0) ? r1 : r2), abs(p.y) - he);
    vec2 cb = p - k1 + k2 * clamp(dot(k1 - p, k2) / dot(k2, k2), 0.0, 1.0);
    float s = (cb.x < 0.0 && ca.y < 0.0) ? -1.0 : 1.0;
    return s * sqrt(min(dot(ca, ca), dot(cb, cb)));
}
float sdEll(vec2 p, vec2 r) { float k0 = length(p / r); float k1 = length(p / (r * r)); return k0 * (k0 - 1.0) / max(k1, 1e-5); }
mat2 rot(float a) { float c = cos(a), s = sin(a); return mat2(c, s, -s, c); }

const int KN_N = 16;
const vec2 KN[KN_N] = vec2[KN_N](
    vec2( 0.200, -0.270), vec2( 0.225, -0.080), vec2( 0.205,  0.080), vec2( 0.150,  0.215),
    vec2( 0.060,  0.300), vec2( 0.010,  0.385), vec2(-0.035,  0.315), vec2(-0.100,  0.275),
    vec2(-0.175,  0.190), vec2(-0.255,  0.075), vec2(-0.262,  0.020), vec2(-0.215, -0.020),
    vec2(-0.140,  0.010), vec2(-0.060,  0.045), vec2(-0.105, -0.090), vec2(-0.165, -0.270));
float sdKnightPoly(vec2 p) {
    float d = dot(p - KN[0], p - KN[0]);
    float s = 1.0;
    int j = KN_N - 1;
    for (int i = 0; i < KN_N; i++) {
        vec2 e = KN[j] - KN[i];
        vec2 w = p - KN[i];
        vec2 b = w - e * clamp(dot(w, e) / dot(e, e), 0.0, 1.0);
        d = min(d, dot(b, b));
        bvec3 c = bvec3(p.y >= KN[i].y, p.y < KN[j].y, e.x * w.y > e.y * w.x);
        if (all(c) || all(not(c))) s *= -1.0;
        j = i;
    }
    return s * sqrt(d);
}

float sdBase(vec2 q) {
    float d = sdRBox(q - vec2(0.0, -0.355), vec2(0.300, 0.048), 0.022);
    d = min(d, sdRBox(q - vec2(0.0, -0.283), vec2(0.235, 0.030), 0.014));
    return d;
}

// signed distance of piece kind k (1..6) silhouette, local coords in squares
// vertical offsets that centre each silhouette's bounding box in its square
// (measured from the shapes; +0.02 lifts them a touch against the heavy bases)
const float PIECE_CY[7] = float[7](0.0, -0.090, -0.029, -0.034, -0.044, -0.033, -0.016);
float pieceSdf(int k, vec2 q) {
    if (abs(q.x) > 0.48 || abs(q.y) > 0.48) return max(abs(q.x), abs(q.y)) - 0.44; // cheap far bound
    q.y += PIECE_CY[k];
    float d = sdBase(q);
    if (k == 1) { // pawn
        d = smin(d, sdTrap(q - vec2(0.0, -0.13), 0.165, 0.072, 0.13), 0.03);
        d = min(d, sdRBox(q - vec2(0.0, 0.015), vec2(0.120, 0.023), 0.014));
        d = smin(d, length(q - vec2(0.0, 0.150)) - 0.112, 0.015);
    } else if (k == 2) { // knight
        float h = sdKnightPoly(q) - 0.012;
        h = max(h, -(length(q - vec2(-0.215, 0.005)) - 0.012)); // nostril notch
        d = smin(d, h, 0.02);
    } else if (k == 3) { // bishop
        d = smin(d, sdTrap(q - vec2(0.0, -0.145), 0.160, 0.066, 0.115), 0.03);
        d = min(d, sdRBox(q - vec2(0.0, -0.010), vec2(0.122, 0.023), 0.014));
        float hd = sdEll(q - vec2(0.0, 0.150), vec2(0.108, 0.150));
        vec2 sq = rot(-0.75) * (q - vec2(0.045, 0.205));
        hd = max(hd, -sdBox(sq, vec2(0.016, 0.085)));
        d = smin(d, hd, 0.015);
        d = min(d, length(q - vec2(0.0, 0.335)) - 0.040);
    } else if (k == 4) { // rook
        d = smin(d, sdTrap(q - vec2(0.0, -0.085), 0.180, 0.140, 0.175), 0.02);
        d = min(d, sdRBox(q - vec2(0.0, 0.112), vec2(0.168, 0.026), 0.010));
        float top = sdRBox(q - vec2(0.0, 0.250), vec2(0.205, 0.105), 0.014);
        float gap = sdBox(vec2(abs(q.x) - 0.068, q.y - 0.345), vec2(0.026, 0.060));
        d = min(d, max(top, -gap));
    } else if (k == 5) { // queen
        d = smin(d, sdTrap(q - vec2(0.0, -0.110), 0.180, 0.078, 0.150), 0.03);
        d = min(d, sdRBox(q - vec2(0.0, 0.055), vec2(0.122, 0.022), 0.014));
        float cr = sdTrap(q - vec2(0.0, 0.165), 0.085, 0.205, 0.090);
        float dip = min(length(vec2(abs(q.x) - 0.052, q.y - 0.272)) - 0.042,
                        length(vec2(abs(q.x) - 0.152, q.y - 0.272)) - 0.042);
        cr = max(cr, -dip);
        d = smin(d, cr, 0.012);
        vec2 b = vec2(abs(q.x), q.y - 0.262);
        float balls = min(length(b - vec2(0.0, 0.0)), min(length(b - vec2(0.102, 0.0)), length(b - vec2(0.200, -0.005)))) - 0.030;
        d = min(d, balls);
        d = min(d, length(q - vec2(0.0, 0.335)) - 0.042);
    } else { // king
        d = smin(d, sdTrap(q - vec2(0.0, -0.110), 0.180, 0.080, 0.150), 0.03);
        d = min(d, sdRBox(q - vec2(0.0, 0.055), vec2(0.124, 0.022), 0.014));
        float cr = sdTrap(q - vec2(0.0, 0.155), 0.088, 0.180, 0.080);
        cr = smin(cr, sdRBox(q - vec2(0.0, 0.240), vec2(0.150, 0.024), 0.016), 0.01);
        d = smin(d, cr, 0.012);
        float cross = min(sdRBox(q - vec2(0.0, 0.330), vec2(0.026, 0.080), 0.008),
                          sdRBox(q - vec2(0.0, 0.338), vec2(0.078, 0.024), 0.008));
        d = min(d, cross);
    }
    return d;
}

// relief height of a piece at signed distance s (s < 0 inside)
float reliefShape(float s) {
    float x = clamp(-s / BEVEL, 0.0, 1.0);
    return PH * sqrt(1.0 - (1.0 - x) * (1.0 - x));
}
float reliefH(float s) {
    float h = reliefShape(s);
    // engraved inner outline gives each piece its drawn "icon" lines
    float groove = 1.0 - smoothstep(0.0, 0.010, abs(s + 0.052));
    return h - groove * 0.055 * step(s, -0.03);
}

int cellPiece(vec2 p) {
    if (p.x < 0.0 || p.y < 0.0 || p.x >= 8.0 || p.y >= 8.0) return 0;
    ivec2 c = ivec2(floor(p));
    return uPieces[c.y * 8 + c.x];
}

// ------------------------------------------------------------- shadow tracing
// ------------------------------------------------------------- shadows
// For shadows every piece is a prism: its silhouette extruded over [z0, z1].
float prismDist(float s, float z, float z0, float z1) {
    float dz = max(z0 - z, z - z1);
    if (dz <= 0.0) return s;
    return length(vec2(max(s, 0.0), dz));
}

// Pieces never leave their cell (minus a margin), so the own cell plus the
// 3 neighbours nearest to p give an exact distance bound.
ivec2 gIgnoreCell = ivec2(-9);
int gIgnoreMover = -1;
float shadowSdf(vec2 p, float z) {
    float d = 0.55;
    vec2 cell = floor(p);
    vec2 nb = sign(p - cell - 0.5);
    for (int j = 0; j < 4; j++) {
        vec2 off = vec2((j == 1 || j == 3) ? nb.x : 0.0, j >= 2 ? nb.y : 0.0);
        vec2 cp = cell + off + 0.5;
        int t = cellPiece(cp);
        if (t > 0 && ivec2(floor(cp)) != gIgnoreCell) {
            // bounding box first: far from the silhouette it is a good enough lower bound
            float bb = sdBox(p - cp, vec2(0.32, 0.44));
            float s = bb > 0.08 ? bb : pieceSdf(t > 6 ? t - 6 : t, p - cp);
            d = min(d, prismDist(s, z, -1.0, PH));
        }
    }
    for (int i = 0; i < 4; i++) {
        if (uMov[i].w < 0.5 || i == gIgnoreMover) continue;
        int mt = int(uMov[i].w + 0.5);
        float sc = uMovScale[i];
        vec2 q = (p - uMov[i].xy) / sc;
        float bb = sdBox(q, vec2(0.32, 0.44));
        float s = (bb > 0.08 ? bb : pieceSdf(mt > 6 ? mt - 6 : mt, q)) * sc;
        d = min(d, prismDist(s, z, uMov[i].z, uMov[i].z + PH * sc));
    }
    return d;
}

// Soft shadow: sphere-trace the prism scene towards the light.
float traceShadow(vec3 P, vec3 L, float k) {
    vec3 d = L - P;
    float dl = length(d.xy);
    if (dl < 1e-3) return 1.0;
    vec2 dir = d.xy / dl;
    float sl = d.z / dl;
    if (sl <= 0.0) return 1.0;
    float tmax = min(dl, (uTopZ + 0.06 - P.z) / sl);
    float res = 1.0;
    float t = 0.004;
    for (int i = 0; i < 64; i++) {
        if (t >= tmax) break;
        vec2 p = P.xy + dir * t;
        float h = shadowSdf(p, P.z + sl * t);
        res = min(res, k * h / t);
        if (res < 0.001) return 0.0;
        t += clamp(h, 0.004, 0.45);
    }
    res = clamp(res, 0.0, 1.0);
    return res * res * (3.0 - 2.0 * res);
}

// ------------------------------------------------------------- lighting
float D_GGX(float NoH, float a) { float a2 = a * a; float d = NoH * NoH * (a2 - 1.0) + 1.0; return a2 / (PI * d * d); }
vec3 F_Schlick(float u, vec3 f0) { return f0 + (1.0 - f0) * pow(1.0 - u, 5.0); }

struct Mat { vec3 albedo; float rough; vec3 f0; float sss; };

// what the sky looks like in a reflection: clouds by day, stars by night, grey in rain
vec3 sky(vec3 r) {
    float up = clamp(r.z, 0.0, 1.0);
    vec2 cp = r.xy / max(r.z, 0.1) * 0.9 + vec2(uTime * 0.012, uTime * 0.005);
    float cl = smoothstep(0.30, 0.62, 0.5 * vnoise(cp) + 0.3 * vnoise(cp * 2.1 + 3.0) + 0.2 * vnoise(cp * 4.3 + 7.0));
    vec3 day = mix(vec3(0.55, 0.70, 0.88), vec3(0.26, 0.46, 0.80), up);
    day = mix(day, vec3(1.0), cl * 0.75);
    vec3 dusk = mix(vec3(1.0, 0.56, 0.34), vec3(0.50, 0.36, 0.62), up) * (0.75 + 0.45 * cl);
    day = mix(day, dusk, uDusk * 0.85);
    day = mix(day, vec3(0.50, 0.53, 0.58) * (0.75 + 0.35 * cl), uRain * 0.85);
    vec3 night = mix(vec3(0.010, 0.016, 0.035), vec3(0.03, 0.035, 0.05), cl * (0.4 + 0.6 * uRain));
    vec2 sp = r.xy / max(r.z, 0.1) * 26.0;
    float sh = hash12(floor(sp));
    float star = step(0.993, sh) * smoothstep(0.22, 0.0, length(fract(sp) - 0.5)) * (1.0 - cl) * (1.0 - uRain);
    night += vec3(0.85, 0.9, 1.0) * star * (0.6 + 1.2 * fract(sh * 91.0));
    return mix(day, night, uNight) + vec3(0.75, 0.82, 1.0) * uFlash * 2.5;
}

// sky without clouds: enough for the faint reflections on wood and pieces
vec3 skyLite(vec3 r) {
    vec3 day = mix(vec3(0.62, 0.74, 0.88), vec3(0.40, 0.56, 0.84), clamp(r.z, 0.0, 1.0));
    day = mix(day, mix(vec3(1.0, 0.58, 0.38), vec3(0.52, 0.38, 0.62), clamp(r.z, 0.0, 1.0)), uDusk * 0.85);
    day = mix(day, vec3(0.55, 0.57, 0.62), uRain * 0.85);
    return mix(day, vec3(0.015, 0.02, 0.04), uNight) + vec3(0.75, 0.82, 1.0) * uFlash * 2.5;
}

// fall < 0: directional light (Lpos far away, no falloff); fall >= 1: short range glow
vec3 shadeLight(vec3 P, vec3 N, vec3 V, Mat m, vec3 Lpos, vec3 Lcol, float shadowK, bool doShadow, float fall) {
    if (max(Lcol.r, max(Lcol.g, Lcol.b)) < 1e-4) return vec3(0.0);
    vec3 Lv = Lpos - P;
    float dist2 = dot(Lv, Lv);
    if (fall >= 1.0 && dist2 > 9.0) return vec3(0.0);
    vec3 L = Lv * inversesqrt(dist2);
    float NoL = dot(N, L);
    float wrap = clamp((NoL + m.sss) / (1.0 + m.sss), 0.0, 1.0);
    if (wrap <= 0.0) return vec3(0.0);
    float att = fall < 0.0 ? 1.0
              : fall >= 1.0 ? smoothstep(9.0, 0.0, dist2) / (1.0 + dist2 * fall)
              : 1.0 / (1.0 + dist2 * max(fall, 0.02));
    float sh = 1.0;
    if (doShadow) sh = traceShadow(P + N * 0.004, Lpos, shadowK);
    vec3 H = normalize(L + V);
    float NoH = max(dot(N, H), 0.0);
    float a = m.rough * m.rough;
    vec3 F = F_Schlick(max(dot(H, V), 0.0), m.f0);
    vec3 spec = uOpt.w > 0.5 ? D_GGX(NoH, a) * F * 0.25 * max(NoL, 0.0) : vec3(0.0);
    vec3 diff = m.albedo * (1.0 - F) / PI * wrap;
    return (diff + spec) * Lcol * sh * att;
}

// sun / moon, lightning, lanterns & fireflies, selection and check glows
vec3 allLights(vec3 P, vec3 N, vec3 V, Mat m, float k, bool sh) {
    vec3 c = shadeLight(P, N, V, m, P + uSun.xyz * 40.0, uSunCol * uSun.w, k, sh, -1.0);
    if (uFlash > 0.01)
        c += shadeLight(P, N, V, m, P + normalize(vec3(0.35, -0.30, 1.0)) * 40.0, vec3(0.75, 0.85, 1.0) * 6.0 * uFlash, 50.0, sh, -1.0);
    for (int i = 0; i < 8; i++) {
        if (i >= uNPL) break;
        if (uPL[i].w < 0.01) continue;
        vec3 d = uPL[i].xyz - P;
        float r2 = uPLCol[i].w * uPLCol[i].w;
        float d2 = dot(d, d);
        if (d2 > r2) continue;
        float fade = smoothstep(r2, r2 * 0.3, d2);
        c += shadeLight(P, N, V, m, uPL[i].xyz, uPLCol[i].rgb * uPL[i].w * fade, k * 0.8, sh && i < uNLantern, 0.35);
    }
    if (uGlow0.w > 0.0) c += shadeLight(P, N, V, m, uGlow0.xyz, uGlow0Col * uGlow0.w, k * 0.6, sh, 3.0);
    if (uGlow1.w > 0.0) c += shadeLight(P, N, V, m, uGlow1.xyz, uGlow1Col * uGlow1.w, k * 0.6, sh, 3.0);
    return c;
}

float frameH(vec2 b) {
    float fe = max(abs(b.x - 4.0), abs(b.y - 4.0)) - 4.0;
    float x = clamp(fe / FRAME, 0.0, 1.0);
    return 0.09 * smoothstep(0.0, 0.16, x) * smoothstep(1.0, 0.80, x) - 0.012 * smoothstep(0.012, 0.0, abs(fe - 0.10));
}

// ------------------------------------------------------------- materials
vec3 woodGrain(vec2 p, vec3 c0, vec3 c1, float dirRot) {
    vec2 q = rot(dirRot) * p;
    float warp = fbm(vec2(q.x * 1.6, q.y * 0.30)) * 2.2;
    float rings = 0.5 + 0.5 * sin((q.x * 3.2 + warp) * 6.2832);
    rings = smoothstep(0.05, 0.95, rings);
    float fine = vnoise(vec2(q.x * 16.0, q.y * 0.7));
    float fig = fbm(q * 2.0 + 5.0);
    return mix(c0, c1, clamp(rings * 0.55 + fine * 0.30 + (fig - 0.5) * 0.4, 0.0, 1.0));
}

vec3 pieceAlbedo(bool white, vec2 q, float s) {
    if (white) {
        float v = fbm(q * 9.0 + 3.0);
        vec3 ivory = mix(vec3(0.86, 0.80, 0.68), vec3(0.97, 0.93, 0.85), v);
        float vein = smoothstep(0.03, 0.0, abs(fbm(q * 5.0 + vec2(7.0)) - 0.5));
        return mix(ivory, vec3(0.78, 0.70, 0.58), vein * 0.35);
    }
    float v = fbm(q * 8.0 + 11.0);
    return mix(vec3(0.030, 0.028, 0.032), vec3(0.075, 0.062, 0.055), v);
}

// raindrops beading on wet surfaces: (normal tilt, coverage)
vec3 drops(vec2 p) {
    if (uRain < 0.02) return vec3(0.0);
    vec2 g = p * 4.2;
    vec2 id = floor(g);
    vec2 f = fract(g) - 0.5;
    float h = hash12(id + 17.0);
    if (h > uRain * 0.32) return vec3(0.0);
    vec2 o = (vec2(hash12(id + 3.1), hash12(id + 7.7)) - 0.5) * 0.5;
    float life = fract(uTime * 0.12 + h * 7.3);
    float r = (0.10 + 0.16 * fract(h * 31.7)) * smoothstep(0.0, 0.05, life) * smoothstep(1.0, 0.75, life);
    vec2 d = (f - o) / max(r, 1e-3);
    if (dot(d, d) >= 1.0) return vec3(0.0);
    return vec3(d * 0.8, 1.0);
}

// ------------------------------------------------------------- pond
vec2 hash22(vec2 p) { return vec2(hash12(p), hash12(p + 19.19)); }

// cellular noise: x = F1, y = F2, z = id of the nearest cell; feature points drift with t
vec3 voronoi(vec2 p, float t) {
    vec2 i = floor(p), f = fract(p);
    float f1 = 8.0, f2 = 8.0, id = 0.0;
    for (int y = -1; y <= 1; y++)
        for (int x = -1; x <= 1; x++) {
            vec2 g = vec2(x, y);
            vec2 h = hash22(i + g);
            vec2 o = 0.5 + 0.42 * sin(t + 6.2831 * h);
            float d = length(g + o - f);
            if (d < f1) { f2 = f1; f1 = d; id = h.x; }
            else if (d < f2) f2 = d;
        }
    return vec3(f1, f2, id);
}

// light focused by the wavy surface: bright filaments where cells meet
float caustics(vec2 p) {
    vec3 a = voronoi(p * 3.1 + 0.4 * vec2(vnoise(p * 1.3 + uTime * 0.2), vnoise(p * 1.3 - uTime * 0.17)), uTime * 0.8);
    float c = exp(-(a.y - a.x) * 6.5);
    return c * c;
}

// Gradient of the water height field, computed in one pass:
// analytic for the swell, ripples and rain rings, finite differences only for the noise.
float swellNoise(vec2 p) {
    float t = uTime;
    return 0.010 * (vnoise(p * 1.6 + vec2(t * 0.22, t * 0.15)) - 0.5)
         + 0.005 * (vnoise(p * 3.7 - vec2(t * 0.30, -t * 0.18)) - 0.5);
}
// radial wave packet A*sin(k*w)*exp(-a*w^2), w = |p - c| - r: returns its gradient
vec2 ringGrad(vec2 p, vec2 c, float r, float A, float k, float a) {
    vec2 d = p - c;
    float l = max(length(d), 1e-4);
    float w = l - r;
    float g = exp(-a * w * w);
    float dv = A * g * (k * cos(k * w) - 2.0 * a * w * sin(k * w));
    return dv * d / l;
}
vec2 waterGrad(vec2 p) {
    float t = uTime;
    vec2 d1 = vec2(0.8, 0.6), d2 = vec2(-0.45, 0.89);
    vec2 g = 0.006 * 2.7 * cos(dot(p, d1) * 2.7 + t * 1.2) * d1
           + 0.004 * 4.3 * cos(dot(p, d2) * 4.3 + t * 1.6) * d2;
    float e = 0.01;
    float n0 = swellNoise(p);
    g += vec2(swellNoise(p + vec2(e, 0.0)) - n0, swellNoise(p + vec2(0.0, e)) - n0) / e;
    g *= 1.0 + uRain * 0.8;
    for (int i = 0; i < 10; i++) {
        float amp = uRip[i].w;
        if (amp <= 0.0) continue;
        float age = t - uRip[i].z;
        if (age < 0.0 || age > 6.0) continue;
        float d = length(p - uRip[i].xy);
        g += ringGrad(p, uRip[i].xy, age * 0.85, amp * exp(-age * 0.6) / (1.0 + d * 1.2), 16.0, 6.0);
    }
    if (uRain > 0.01) {
        vec2 i0 = floor(p / 0.6);
        for (int y = -1; y <= 1; y++)
            for (int x = -1; x <= 1; x++) {
                vec2 c = i0 + vec2(x, y);
                float h = hash12(c + 41.0);
                if (h > uRain) continue;
                float per = 0.9 + 0.7 * fract(h * 13.1);
                float cyc = t / per + h * 5.7;
                float ph = fract(cyc);
                vec2 ctr = (c + 0.2 + 0.6 * hash22(c + floor(cyc))) * 0.6;
                g += ringGrad(p, ctr, ph * 0.5, 0.004 * (1.0 - ph) * (1.0 - ph), 55.0, 900.0);
            }
    }
    return g;
}

float bottomZ(vec2 p) { return WZ - 0.42 - 0.22 * vnoise(p * 0.45); }

// rounded pebbles of mixed size in sand, with algae
vec3 pebbles(vec2 p) {
    vec2 w = p + 0.25 * vec2(vnoise(p * 1.3), vnoise(p * 1.3 + 5.0));   // break the cell regularity
    vec3 v = voronoi(w * 3.1, 0.0);
    vec3 v2 = voronoi(w * 6.3 + 11.0, 0.0);
    float big = smoothstep(0.05, 0.16, v.y - v.x) * smoothstep(0.62, 0.28, v.x);
    float small = smoothstep(0.05, 0.14, v2.y - v2.x) * smoothstep(0.55, 0.25, v2.x) * (1.0 - big);
    float h = big > 0.0 ? v.z : v2.z;
    vec3 stone = mix(vec3(0.50, 0.46, 0.40), vec3(0.30, 0.31, 0.28), h);
    stone = mix(stone, vec3(0.62, 0.55, 0.45), step(0.80, h));
    stone = mix(stone, vec3(0.36, 0.28, 0.22), step(h, 0.12));
    float dome = big > 0.0 ? 1.0 - v.x * 1.2 : 1.0 - v2.x * 1.4;          // rounded top catches light
    stone *= 0.65 + 0.45 * clamp(dome, 0.0, 1.0);
    float sandy = smoothstep(0.50, 0.68, fbm(p * 0.55 + 3.0));             // sandy patches
    vec3 sand = mix(vec3(0.46, 0.41, 0.31), vec3(0.38, 0.34, 0.26), vnoise(p * 9.0));
    vec3 c = mix(sand * 0.75, stone, max(big, small) * (1.0 - sandy * 0.85));
    return mix(c, vec3(0.18, 0.28, 0.12), smoothstep(0.58, 0.82, fbm(p * 0.7)) * 0.6);
}

const int NPADS = 9;
const vec4 PADS[NPADS] = vec4[NPADS](
    vec4(-1.30, -0.55, 0.36, 0.4), vec4(-3.05, 3.40, 0.42, 2.1), vec4(-2.95, 8.95, 0.34, 4.0),
    vec4( 9.65, -0.60, 0.40, 1.2), vec4(11.00, 4.30, 0.40, 5.2), vec4(10.90, 8.85, 0.42, 3.0),
    vec4( 2.40, -1.25, 0.34, 0.9), vec4( 6.70, 9.30, 0.38, 2.6), vec4( 4.90, 9.55, 0.30, 5.9));

float padSdf(vec2 p, int i, out vec2 q) {
    vec4 pd = PADS[i];
    vec2 c = pd.xy + 0.04 * vec2(sin(uTime * 0.35 + float(i)), cos(uTime * 0.28 + float(i) * 2.0));
    q = rot(-(pd.w + 0.12 * sin(uTime * 0.15 + float(i)))) * (p - c);
    float d = length(q) - pd.z;
    float notch = (abs(q.y) - q.x * 0.28) * 0.96;
    return max(d, -notch);
}

vec4 lotus(vec2 q) {
    float r = length(q), a = atan(q.y, q.x);
    float outer = 0.16 * (0.55 + 0.45 * pow(abs(cos(a * 4.0)), 0.6));
    if (r > outer) return vec4(0.0);
    float inner = 0.10 * (0.55 + 0.45 * pow(abs(cos(a * 4.0 + 0.785)), 0.6));
    vec3 c = mix(vec3(0.95, 0.50, 0.68), vec3(1.0, 0.92, 0.95), smoothstep(outer, outer * 0.3, r));
    c *= 0.8 + 0.2 * smoothstep(0.0, 0.5, abs(cos(a * 4.0)));
    if (r < inner) c = mix(vec3(1.0, 0.72, 0.84), vec3(1.0, 0.97, 0.98), smoothstep(inner, 0.0, r));
    if (r < 0.035) c = vec3(1.0, 0.80, 0.25);
    return vec4(c, 1.0);
}

// koi body half-width at local x (snout at +0.30, tail root at -0.245)
float koiWidth(float x) {
    float v = clamp((0.30 - x) / 0.545, 0.0, 1.0);          // 0 snout .. 1 tail root
    return 0.080 * pow(sin(3.14159 * pow(v, 0.62)), 0.7) + 0.024 * v * v;
}

// A koi seen from above, local frame x = forward. part: 0 body, 1 paired fins, 2 tail fin.
float fishSdf(vec2 p, int i, out vec2 lq, out float part) {
    vec4 f = uFish[i];
    float sz = uFishK[i].y;
    vec2 rel = (p - f.xy) / sz;
    part = 0.0;
    lq = rel;
    float r2 = dot(rel, rel);
    if (r2 > 0.36) return (sqrt(r2) - 0.55) * sz;           // far: skip the rest
    vec2 q = rot(-f.z) * rel;
    // a travelling wave runs down the spine, growing towards the tail; turning curves the body
    float tw = smoothstep(0.22, -0.48, q.x);
    q.y -= sin(f.w - q.x * 6.5) * (0.010 + 0.075 * tw * tw) + uFishK[i].z * (0.30 - q.x) * (0.30 - q.x);
    lq = q;
    float body = max((abs(q.y) - koiWidth(q.x)) * 0.85, max(q.x - 0.30, -0.245 - q.x));
    // tail fin: a wide flowing fan with a forked trailing edge
    vec2 t = vec2(q.x + 0.235, q.y);
    float hw = 0.022 + max(-t.x, 0.0) * 0.72;
    float tail = max(max(abs(t.y) - hw, t.x - 0.01), -t.x - (0.15 + 0.55 * abs(t.y))) * 0.8;
    // pectoral fins (front pair, flapping) and pelvic fins (small, further back)
    float flap = 0.85 + 0.3 * sin(f.w * 0.45);
    vec2 pq = vec2(q.x - 0.115, abs(q.y) - koiWidth(0.115) - 0.012);
    float pect = sdEll(rot(flap) * (pq - vec2(-0.03, 0.03)), vec2(0.065, 0.028));
    vec2 vq = vec2(q.x + 0.06, abs(q.y) - koiWidth(-0.06) + 0.005);
    float pelv = sdEll(rot(0.9) * (vq - vec2(-0.02, 0.018)), vec2(0.038, 0.016));
    float fins = min(pect, pelv);
    part = body < 0.0 ? 0.0 : (tail < fins ? 2.0 : 1.0);
    return min(body, min(tail, fins)) * sz;
}

// koi skin: ragged colour patches, scales, dorsal fin line. px = one pixel in fish units
vec3 koiSkin(int kind, vec2 q, float seed, float px) {
    vec2 wq = q * 6.0 + seed * 7.3;
    float n = fbm(wq + 0.6 * vec2(vnoise(wq * 2.0), vnoise(wq * 2.0 + 4.0)));
    float n2 = fbm(q * 9.0 + seed * 3.1 + 11.0);
    vec3 white = vec3(0.96, 0.94, 0.90), hi = vec3(0.88, 0.23, 0.06), sumi = vec3(0.05, 0.05, 0.06);
    float w = max(koiWidth(q.x), 0.01);
    vec3 c;
    float net = 0.18;
    if (kind == 0) c = mix(white, hi, smoothstep(0.50, 0.53, n));                                   // kohaku
    else if (kind == 1) c = vec3(1.0, 0.74, 0.30) * (0.9 + 0.2 * n2);                               // yamabuki ogon
    else if (kind == 2) { c = mix(sumi, hi, smoothstep(0.52, 0.55, n)); c = mix(c, white, smoothstep(0.58, 0.61, n2)); } // showa
    else if (kind == 3) c = mix(white, hi, smoothstep(0.075, 0.065, length((q - vec2(0.17, 0.0)) * vec2(1.0, 1.3)))); // tancho
    else if (kind == 4) {                                                                           // asagi
        c = mix(vec3(0.42, 0.52, 0.64), vec3(0.95, 0.48, 0.18), smoothstep(0.35, 0.85, abs(q.y) / w) * 0.8);
        net = 0.38;
    } else {                                                                                        // sanke
        c = mix(white, hi, smoothstep(0.51, 0.54, n));
        c = mix(c, sumi, smoothstep(0.66, 0.69, n2) * step(q.x, 0.2));
    }
    // scales: staggered rows of arcs, faded when they would be smaller than ~3 px
    vec2 sg = q * vec2(26.0, 30.0);
    sg.y += 0.5 * floor(sg.x);
    float arc = smoothstep(0.40, 0.5, length(fract(sg) - vec2(0.3, 0.5)));
    float vis = smoothstep(2.0, 4.0, (1.0 / 26.0) / px);
    c *= 1.0 - arc * net * vis;
    // dorsal fin: a thin darker ridge along the spine
    c *= 1.0 - 0.25 * smoothstep(0.012, 0.0, abs(q.y)) * step(-0.12, q.x) * step(q.x, 0.08);
    return c;
}

// captured pieces on the bottom: 2 columns x 8 rows per side (white losses left, black right)
vec2 slotCentre(int s) {
    int side = s / 16;
    int i = s - side * 16;
    float col = float(i / 8), row = float(i - (i / 8) * 8);
    return vec2(side == 0 ? -1.35 - col * 0.72 : 9.35 + col * 0.72, 7.45 - row * 0.98);
}
int sunkPiece(vec2 p, out vec2 q) {
    int side = p.x < 4.0 ? 0 : 1;
    float cx = side == 0 ? (-1.35 - p.x) / 0.72 : (p.x - 9.35) / 0.72;
    float col = floor(cx + 0.5);
    float row = floor((7.45 - p.y) / 0.98 + 0.5);
    if (col < 0.0 || col > 1.0 || row < 0.0 || row > 7.0) return 0;
    int s = side * 16 + int(col) * 8 + int(row);
    if (s == uSinkSlot) return 0;
    int t = uSunk[s];
    if (t == 0) return 0;
    float ang = (hash12(vec2(float(s), 3.7)) - 0.5) * 1.6;
    q = rot(-ang) * (p - slotCentre(s)) / 0.8;
    return t;
}

// sunlight (or moonlight) arriving at an underwater point, with caustics and shadows
vec3 underLight(vec2 x, float z) {
    vec3 sd = uSun.xyz;
    float depth = WZ - z;
    vec2 sp = x + sd.xy / sd.z * depth;            // where its ray crossed the surface
    vec2 qb = abs(sp - 4.0) - vec2(4.0 + FRAME);
    float dr = length(max(qb, 0.0)) + min(max(qb.x, qb.y), 0.0);
    float s = smoothstep(-0.10, 0.22, dr);         // the raft
    vec2 q;
    for (int i = 0; i < NPADS; i++) {
        vec2 dd = sp - PADS[i].xy;
        if (dot(dd, dd) > (PADS[i].z + 0.15) * (PADS[i].z + 0.15)) continue;
        s *= mix(0.25, 1.0, smoothstep(-0.02, 0.06, padSdf(sp, i, q)));
    }
    for (int i = 0; i < 6; i++) {
        if (i >= uNFish) break;
        float zf = WZ - uFishK[i].w;
        if (zf <= z + 0.01) continue;                // only fish above the point cast onto it
        vec2 fp = x + sd.xy / sd.z * (zf - z);
        float part;
        float fs = fishSdf(fp, i, q, part);
        s *= mix(part > 0.5 ? 0.65 : 0.3, 1.0, smoothstep(-0.01, 0.06 + 0.1 * (zf - z), fs));
    }
    float c = uNight > 0.95 ? 0.75 : 0.55 + 0.9 * caustics(sp) * (1.0 - 0.7 * uRain) * (1.0 - uNight);
    vec3 L = uSunCol * uSun.w * sd.z * exp(-ABSORB * depth / sd.z) * s * c / PI;
    L += vec3(0.75, 0.85, 1.0) * 6.0 * uFlash * exp(-ABSORB * depth) / PI;
    return L;
}

// lanterns and fireflies lighting the pond bed
vec3 pointUnder(vec2 x, float z) {
    vec3 c = vec3(0.0);
    for (int i = 0; i < 8; i++) {
        if (i >= uNPL) break;
        float I = uPL[i].w;
        if (I < 0.01) continue;
        vec3 d = uPL[i].xyz - vec3(x, z);
        float d2 = dot(d, d);
        float r2 = uPLCol[i].w * uPLCol[i].w;
        if (d2 > r2) continue;
        c += uPLCol[i].rgb * I * (d.z * inversesqrt(d2)) / (1.0 + d2 * 0.8) * smoothstep(r2, r2 * 0.3, d2);
    }
    return c * exp(-ABSORB * (WZ - z) * 1.2) / PI;
}

vec3 underwaterPiece(int t, vec2 q, float s, vec2 x, float z) {
    int kk = t > 6 ? t - 6 : t;
    bool white = t <= 6;
    float e = 0.004;
    float h0 = reliefH(s);
    vec3 N = normalize(vec3(-(reliefH(pieceSdf(kk, q + vec2(e, 0.0))) - h0) / e,
                            -(reliefH(pieceSdf(kk, q + vec2(0.0, e))) - h0) / e, 0.8));
    vec3 alb = pieceAlbedo(white, q, s);
    if (!white) alb = alb * 2.2 + 0.025;          // keep ebony readable under water
    vec3 L = underLight(x, z);
    float ndl = max(dot(N, normalize(vec3(uSun.xy * 0.5, 1.0))), 0.0);
    vec3 c = alb * (L * (0.4 + 0.8 * ndl) + gAmb * 0.12 * exp(-ABSORB * (WZ - z)));
    c += alb * pointUnder(x, z);
    c += L * 0.25 * pow(max(N.z, 0.0), 40.0) * (white ? 0.3 : 0.8);
    return c;
}

vec3 staticAt(vec2 bp) {
    float sqPx = uBoard.z / 8.0;
    vec2 pxl = vec2(uBoard.x + bp.x * sqPx, uBoard.y + (8.0 - bp.y) * sqPx);
    return texture(uStatic, vec2(pxl.x, uRes.y - pxl.y) / uRes).rgb;
}

vec3 bottomColor(vec2 pb, float zb) {
    vec2 q;
    float aoP = 1.0;
    int t = sunkPiece(pb, q);
    if (t > 0) {
        float s = pieceSdf(t > 6 ? t - 6 : t, q);
        if (s < 0.0) return underwaterPiece(t, q, s, pb, zb + 0.02);
        aoP = 1.0 - 0.5 * exp(-s * 18.0);
    }
    vec3 alb = staticAt(pb);
    vec3 c = alb * (underLight(pb, zb) + gAmb * 0.12 * exp(-ABSORB * (WZ - zb))) * aoP;
    return c + alb * pointUnder(pb, zb) * aoP;
}

vec3 shadeWater(vec2 p, inout vec3 emit) {
    vec3 V = vec3(0.0, 0.0, 1.0);
    vec2 qb = abs(p - 4.0) - vec2(4.0 + FRAME);
    float dRaft = length(max(qb, 0.0)) + min(max(qb.x, qb.y), 0.0);

    // ---- floating paper lanterns
    for (int i = 0; i < 3; i++) {
        if (i >= uNLantern) break;
        float I = uPL[i].w;
        if (I < 0.01) continue;
        vec2 q = rot(float(i) * 1.3 + uTime * 0.04) * (p - uPL[i].xy);
        float d = sdRBox(q, vec2(0.17), 0.035);
        if (d < 0.0) {
            vec3 c = vec3(1.0, 0.55, 0.20) * (1.6 + 2.8 * exp(-length(q) * 10.0)) * I * 0.45;
            float wood = max(smoothstep(-0.028, -0.016, d), 1.0 - smoothstep(0.006, 0.011, min(abs(q.x), abs(q.y))));
            emit += mix(c, vec3(0.10, 0.05, 0.02) * (0.3 + I * 0.3), wood * 0.85);
            return vec3(0.0);
        }
    }
    // ---- lily pads and lotus flowers
    for (int i = 0; i < NPADS; i++) {
        vec2 dd = p - PADS[i].xy;
        if (dot(dd, dd) > (PADS[i].z + 0.08) * (PADS[i].z + 0.08)) continue;
        vec2 q;
        float d = padSdf(p, i, q);
        if (d < 0.0) {
            float r = length(q) / PADS[i].z;
            float a = atan(q.y, q.x);
            vec3 g = mix(vec3(0.09, 0.26, 0.07), vec3(0.20, 0.42, 0.11), fbm(q * 5.0 + float(i) * 3.0));
            g *= 1.0 - 0.25 * smoothstep(0.75, 1.0, cos(a * 13.0)) * smoothstep(0.1, 0.3, r);
            g = mix(g, vec3(0.32, 0.20, 0.10), smoothstep(-0.03, 0.0, d) * 0.6);
            vec3 N = normalize(vec3(q / PADS[i].z * 0.25 * smoothstep(0.7, 1.0, r), 1.0));
            vec3 dr = drops(p * 1.3);
            N = normalize(N + vec3(dr.xy * dr.z, 0.0));
            Mat m;
            m.albedo = g * (1.0 - 0.3 * uRain); m.rough = mix(0.35, 0.10, uRain); m.f0 = vec3(0.05); m.sss = 0.2;
            vec3 c = allLights(vec3(p, WZ + 0.01), N, V, m, uOpt.y, false) + m.albedo * gAmb * 0.8;
            if (i == 0 || i == 5 || i == 7) {
                vec4 lo = lotus(q - vec2(-0.06, 0.03));
                if (lo.a > 0.0) {
                    Mat lm;
                    lm.albedo = lo.rgb; lm.rough = 0.4; lm.f0 = vec3(0.04); lm.sss = 0.5;
                    c = allLights(vec3(p, WZ + 0.08), V, V, lm, uOpt.y, false) + lo.rgb * gAmb * 0.8;
                }
            }
            return c;
        }
    }
    // ---- koi food floating on the surface
    for (int i = 0; i < 6; i++) {
        if (uFood[i].z < 0.5) continue;
        vec2 d = p - uFood[i].xy;
        if (dot(d, d) < 0.035 * 0.035) {
            Mat m;
            m.albedo = vec3(0.45, 0.28, 0.12); m.rough = 0.5; m.f0 = vec3(0.04); m.sss = 0.0;
            return allLights(vec3(p, WZ), normalize(vec3(d / 0.035 * 0.7, 1.0)), V, m, uOpt.y, false) + m.albedo * gAmb * 0.8;
        }
    }

    // ---- the water surface
    vec3 N = normalize(vec3(-waterGrad(p), 1.0));
    vec3 rd = refract(vec3(0.0, 0.0, -1.0), N, 0.75);
    float zb = bottomZ(p);
    vec2 pb = p + rd.xy * ((WZ - zb) / -rd.z);
    float viewLen = (WZ - zb) / -rd.z;
    vec3 under = vec3(0.0);
    bool hit = false;

    // a captured piece still sinking to the bottom, tumbling as it goes
    if (uSinkSlot >= 0) {
        vec2 c = slotCentre(uSinkSlot);
        float e2 = smoothstep(0.0, 1.0, uSinkT);
        float zs = mix(WZ - 0.04, bottomZ(c) + 0.03, e2);
        vec2 ps = p + rd.xy * ((WZ - zs) / -rd.z);
        float ang = (hash12(vec2(float(uSinkSlot), 3.7)) - 0.5) * 1.6 * e2 + sin(uSinkT * 9.0) * 0.15 * (1.0 - e2);
        vec2 q = rot(-ang) * (ps - c) / 0.8;
        int t = uSunk[uSinkSlot];
        if (t > 0) {
            float s = pieceSdf(t > 6 ? t - 6 : t, q);
            if (s < 0.0) {
                under = underwaterPiece(t, q, s, ps, zs);
                viewLen = (WZ - zs) / -rd.z;
                hit = true;
            }
        }
        // bubbles rising from it
        for (int j = 0; j < 5; j++) {
            float ph = fract(uSinkT * 2.2 + float(j) * 0.21);
            vec2 bp = c + (hash22(vec2(float(j), float(uSinkSlot))) - 0.5) * 0.4 + vec2(0.0, ph * 0.05);
            float br = 0.018 + 0.01 * hash12(vec2(float(j), 9.0));
            float bd = abs(length(p - bp) - br * (1.0 - ph * 0.5));
            emit += vec3(0.7, 0.85, 0.9) * (1.0 - smoothstep(0.0, pxSize * 1.5, bd)) * 0.25 * (1.0 - uSinkT) * (gAmb.b * 2.0 + 0.2);
        }
    }
    // koi, each at its own depth
    if (!hit) {
        for (int i = 0; i < 6; i++) {
            if (i >= uNFish) break;
            float zf = WZ - uFishK[i].w;
            vec2 pf = p + rd.xy * ((WZ - zf) / -rd.z);
            vec2 lq;
            float part;
            float s = fishSdf(pf, i, lq, part);
            if (s < 0.0) {
                int kind = int(uFishK[i].x + 0.5);
                vec3 L = underLight(pf, zf) + pointUnder(pf, zf);
                vec3 ld = normalize(vec3(uSun.xy * 0.6, 1.0));
                vec3 c;
                if (part < 0.5) {
                    float w = max(koiWidth(lq.x), 0.01);
                    float ny = clamp(lq.y / w, -1.0, 1.0);
                    vec3 n = normalize(vec3(rot(uFish[i].z) * vec2(0.0, ny * 0.85), sqrt(max(1.0 - ny * ny * 0.72, 0.05))));
                    vec3 sk = koiSkin(kind, lq, float(i), pxSize / uFishK[i].y);
                    c = sk * (L * (0.30 + 0.85 * max(dot(n, ld), 0.0)) + gAmb * 0.2 * exp(-ABSORB * uFishK[i].w));
                    float sp = pow(max(dot(reflect(-V, n), ld), 0.0), kind == 1 ? 12.0 : 30.0);
                    c += L * sp * (kind == 1 ? 0.9 : 0.35);
                    // eyes with a tiny highlight
                    vec2 e = vec2(lq.x - 0.235, abs(lq.y) - koiWidth(0.235) * 0.72);
                    if (length(e) < 0.013) c = mix(vec3(0.02), vec3(0.9), smoothstep(0.005, 0.002, length(e - vec2(0.003, 0.004))) * 0.6);
                } else {
                    // translucent fins with rays: the bed shows through
                    vec3 tint = kind == 2 ? vec3(0.18, 0.18, 0.20) : kind == 1 ? vec3(1.0, 0.78, 0.40) : kind == 4 ? vec3(0.95, 0.50, 0.20) : vec3(0.95, 0.93, 0.90);
                    vec2 t = lq - vec2(part > 1.5 ? -0.235 : 0.115, 0.0);
                    float rays = 0.75 + 0.25 * sin(atan(t.y, -t.x) * 46.0);
                    float alpha = part > 1.5 ? 0.5 - 0.25 * smoothstep(0.08, 0.2, -t.x) : 0.55;
                    c = mix(bottomColor(pb, zb), tint * rays * (L * 0.9 + gAmb * 0.2), alpha);
                }
                under = c;
                viewLen = (WZ - zf) / -rd.z;
                hit = true;
                break;
            }
        }
    }
    if (!hit) under = bottomColor(pb, zb);

    // absorption and in-scattering along the view path
    vec3 tr = exp(-ABSORB * viewLen);
    vec3 scatter = mix(vec3(0.035, 0.085, 0.055) * (0.3 + 0.1 * uSun.w), vec3(0.003, 0.008, 0.014), uNight) * (1.0 + uFlash * 5.0);
    vec3 col = under * tr + scatter * (1.0 - tr);
    // surface reflection of the sky
    vec3 R = reflect(-V, N);
    float F = 0.02 + 0.98 * pow(1.0 - max(N.z, 0.0), 5.0);
    col = mix(col, sky(R), clamp(F * 2.5 + 0.03, 0.0, 1.0));
    // sun / moon glitter on the wavelets
    float gl = smoothstep(0.93, 1.0, vnoise(p * 26.0 + vec2(uTime * 2.1, -uTime * 1.7))) * smoothstep(0.015, 0.05, length(N.xy));
    emit += uSunCol * uSun.w * gl * 0.12 * (1.0 - uRain * 0.7);
    // lanterns and fireflies mirrored in the ripples
    for (int i = 0; i < 8; i++) {
        if (i >= uNPL) break;
        float I = uPL[i].w;
        if (I < 0.01) continue;
        vec2 d = p - uPL[i].xy - N.xy * 0.8;
        bool lantern = i < uNLantern;
        emit += uPLCol[i].rgb * I * (lantern ? 0.22 : 0.10) * exp(-dot(d, d) * (lantern ? 5.0 : 30.0)) * (0.6 + 4.0 * length(N.xy));
    }
    // shade and a little foam along the raft
    col *= mix(0.45, 1.0, smoothstep(0.0, 0.7, dRaft));
    float foam = smoothstep(0.05, 0.0, dRaft) * (0.4 + 0.6 * vnoise(p * 25.0 + uTime));
    col = mix(col, vec3(0.55, 0.6, 0.6) * (gAmb + uSun.w * 0.06), foam * 0.35);
    return col;
}

// ------------------------------------------------------------- main
void main() {
    vec2 px = vec2(gl_FragCoord.x, uRes.y - gl_FragCoord.y);   // y down, like the UI
    float sqPx = uBoard.z / 8.0;
    vec2 b = vec2((px.x - uBoard.x) / sqPx, 8.0 - (px.y - uBoard.y) / sqPx);
    pxSize = 1.0 / sqPx;
    if (uPass == 1) {
        vec2 fb = b - 4.0;
        float edge = max(abs(fb.x), abs(fb.y));
        vec3 a;
        if (edge <= 4.0) {
            ivec2 c = ivec2(clamp(floor(b), 0.0, 7.0));
            a = ((c.x + c.y) & 1) == 1
                ? woodGrain(b + vec2(c) * 3.1, vec3(0.84, 0.70, 0.50), vec3(0.68, 0.52, 0.34), 0.0)
                : woodGrain(b + vec2(c) * 1.7, vec3(0.30, 0.18, 0.11), vec3(0.17, 0.095, 0.055), 1.5708);
        } else if (edge - 4.0 < FRAME) {
            a = woodGrain(b + 40.0, vec3(0.16, 0.08, 0.045), vec3(0.08, 0.035, 0.02), abs(fb.x) < abs(fb.y) ? 1.5708 : 0.0);
        } else {
            a = pebbles(b);
        }
        fragColor = vec4(a, 1.0);
        return;
    }
    vec3 dayAmb = mix(mix(vec3(0.20, 0.23, 0.30), vec3(0.24, 0.17, 0.20), uDusk * 0.8), vec3(0.24, 0.25, 0.28), uRain);
    gAmb = mix(dayAmb, vec3(0.022, 0.032, 0.065), uNight)
         + vec3(0.6, 0.65, 0.8) * uFlash;

    vec3 V = vec3(0.0, 0.0, 1.0);
    float k = uOpt.y * (1.0 - 0.6 * uRain);
    bool shadows = uOpt.x > 0.5;

    // ---- top-most piece under this pixel (lifted / moving pieces first)
    int hitType = 0;
    vec2 hitQ = vec2(0.0);
    float hitZ0 = 0.0, hitScale = 1.0, hitS = 1.0;
    int hitMover = -1;
    for (int i = 0; i < 4; i++) {
        if (uMov[i].w < 0.5 || hitType != 0) continue;
        int mt = int(uMov[i].w + 0.5);
        float sc = uMovScale[i];
        vec2 q = (b - uMov[i].xy) / sc;
        float s = pieceSdf(mt > 6 ? mt - 6 : mt, q);
        if (s < 0.0) { hitType = mt; hitQ = q; hitZ0 = uMov[i].z; hitScale = sc; hitS = s; hitMover = i; }
    }
    if (hitType == 0) {
        int t = cellPiece(b);
        if (t > 0) {
            vec2 q = b - floor(b) - 0.5;
            float s = pieceSdf(t > 6 ? t - 6 : t, q);
            if (s < 0.0) { hitType = t; hitQ = q; hitS = s; }
        }
    }

    vec3 col = vec3(0.0);
    vec3 emit = vec3(0.0);

    if (hitType != 0) {
        // ------------------------------------------------ piece relief
        int kk = hitType > 6 ? hitType - 6 : hitType;
        bool white = hitType <= 6;
        float e = 0.0025;
        float h0 = reliefH(hitS);
        float hx = reliefH(pieceSdf(kk, hitQ + vec2(e, 0.0)));
        float hy = reliefH(pieceSdf(kk, hitQ + vec2(0.0, e)));
        vec3 N = normalize(vec3(-(hx - h0) / e, -(hy - h0) / e, hitScale));
        N = normalize(N + vec3(vnoise(hitQ * 24.0) - 0.5, vnoise(hitQ * 24.0 + 9.0) - 0.5, 0.0) * 0.015);
        vec3 dr = drops(hitQ * 1.4 + float(hitType) * 3.1);
        N = normalize(N + vec3(dr.xy * dr.z, 0.0));
        vec3 P = vec3(b, hitZ0 + h0 * hitScale);
        if (hitMover >= 0) gIgnoreMover = hitMover; else gIgnoreCell = ivec2(floor(b));
        Mat m;
        m.albedo = pieceAlbedo(white, hitQ, hitS) * (1.0 - 0.12 * uRain);
        m.rough = mix(white ? 0.34 : 0.17, 0.07, uRain * 0.8);
        m.f0 = vec3(white ? 0.045 : 0.06);
        m.sss = white ? 0.45 : 0.05;
        col += allLights(P, N, V, m, k, shadows);
        float ao = mix(0.55, 1.0, smoothstep(0.0, PH, h0));
        vec3 Fr = F_Schlick(max(N.z, 0.0), m.f0);
        col += m.albedo * gAmb * ao;
        if (uOpt.w > 0.5) col += skyLite(reflect(-V, N)) * Fr * (white ? 0.35 : 0.45) * ao;
    } else {
        vec2 fb = b - 4.0;
        float edge = max(abs(fb.x), abs(fb.y));
        if (edge <= 4.0) {
            // ------------------------------------------------ squares
            ivec2 c = ivec2(clamp(floor(b), 0.0, 7.0));
            int idx = c.y * 8 + c.x;
            bool light = ((c.x + c.y) & 1) == 1;
            vec2 local = b - floor(b) - 0.5;
            vec3 alb = texelFetch(uStatic, ivec2(gl_FragCoord.xy), 0).rgb;
            int mk = uMarks[idx];
            if ((mk & MK_LAST) != 0) alb = mix(alb, vec3(0.95, 0.75, 0.25), 0.35);
            float seam = min(0.5 - abs(local.x), 0.5 - abs(local.y));
            alb *= mix(0.55, 1.0, smoothstep(0.0, 0.012, seam));
            alb *= 1.0 - 0.22 * uRain;                                   // wet wood is darker
            vec3 N = normalize(vec3((vnoise(b * 40.0) - 0.5) * 0.03, (vnoise(b * 40.0 + 7.0) - 0.5) * 0.03, 1.0));
            vec3 dr = drops(b);
            N = normalize(N + vec3(dr.xy * dr.z, 0.0));
            Mat m;
            m.albedo = alb; m.rough = mix(0.30, 0.07, uRain); m.f0 = vec3(0.04); m.sss = 0.0;
            col += allLights(vec3(b, 0.0), N, V, m, k, shadows);
            // contact ambient occlusion from pieces in this & neighbouring cells
            float ao = 1.0;
            if (uOpt.z > 0.5) {
                vec2 nb = sign(local);
                for (int j = 0; j < 4; j++) {
                    vec2 off = vec2(j == 1 || j == 3 ? nb.x : 0.0, j >= 2 ? nb.y : 0.0);
                    vec2 cp = floor(b) + off + 0.5;
                    int t = cellPiece(cp);
                    if (t > 0) ao *= 1.0 - 0.38 * exp(-max(pieceSdf(t > 6 ? t - 6 : t, b - cp), 0.0) * 14.0);
                }
            }
            col += alb * gAmb * 0.8 * ao;
            col *= mix(1.0, ao, 0.6);
            if (uOpt.w > 0.5) col += skyLite(reflect(-V, N)) * (0.03 + 0.05 * uRain) * ao;

            // ---- marks (emissive)
            float pulse = 0.75 + 0.25 * sin(uTime * 4.0);
            if ((mk & MK_SEL) != 0) {
                float bd = 0.5 - max(abs(local.x), abs(local.y));
                emit += vec3(1.0, 0.65, 0.25) * (exp(-bd * 28.0) * 2.2 + 0.10);
            }
            if ((mk & MK_MOVE) != 0) {
                float hov = (mk & MK_HOVER) != 0 ? 1.6 : 1.0;
                float r = length(local);
                emit += vec3(0.25, 0.85, 1.0) * (smoothstep(0.115, 0.095, r) * 3.0 * pulse + exp(-r * 9.0) * 0.25) * hov;
            }
            if ((mk & MK_CAPT) != 0) {
                float hov = (mk & MK_HOVER) != 0 ? 1.6 : 1.0;
                float cr = max(abs(local.x), abs(local.y));
                emit += vec3(1.0, 0.25, 0.20) * smoothstep(0.40, 0.46, cr) * smoothstep(0.5, 0.47, cr) * 3.0 * pulse * hov;
            }
            if ((mk & MK_CHECK) != 0) {
                float r = length(local);
                emit += vec3(1.0, 0.08, 0.04) * exp(-r * 4.0) * 1.6 * (0.7 + 0.3 * sin(uTime * 6.0));
            }
            if ((mk & MK_HOVER) != 0 && (mk & (MK_MOVE | MK_CAPT)) == 0) col *= 1.12;
        } else if (edge - 4.0 < FRAME) {
            // ------------------------------------------------ raised frame of the raft
            float fe = edge - 4.0;
            float e = 0.004;
            float hgt = frameH(b);
            vec3 N = normalize(vec3(-(frameH(b + vec2(e, 0.0)) - hgt) / e, -(frameH(b + vec2(0.0, e)) - hgt) / e, 1.0));
            vec3 dr = drops(b * 1.1 + 5.0);
            N = normalize(N + vec3(dr.xy * dr.z, 0.0));
            vec3 alb = texelFetch(uStatic, ivec2(gl_FragCoord.xy), 0).rgb;
            float inl = smoothstep(0.012, 0.0, abs(fe - 0.10));
            Mat m;
            m.albedo = mix(alb * (1.0 - 0.2 * uRain), vec3(0.9, 0.65, 0.25), inl);
            m.rough = mix(mix(0.32, 0.08, uRain), 0.22, inl);
            m.f0 = mix(vec3(0.04), vec3(0.95, 0.7, 0.35), inl);
            m.sss = 0.0;
            col += allLights(vec3(b, hgt), N, V, m, k, false);
            col += m.albedo * gAmb * 0.8;
            if (uOpt.w > 0.5) col += skyLite(reflect(-V, N)) * F_Schlick(N.z, m.f0) * 0.5;
        } else {
            col = shadeWater(b, emit);
        }
    }

    // ---- fireflies drift above everything
    for (int i = 0; i < 8; i++) {
        if (i >= uNPL) break;
        if (i < uNLantern || uPL[i].w < 0.01) continue;
        float d = length(b - uPL[i].xy);
        emit += uPLCol[i].rgb * uPL[i].w * (smoothstep(0.03, 0.012, d) * 5.0 + exp(-d * 18.0) * 0.5);
    }
    // ---- falling rain streaks
    if (uRain > 0.01) {
        vec2 q = rot(0.25) * b;
        float colw = 1.0 / 9.0;
        float cx = floor(q.x / colw);
        float hh = hash12(vec2(cx, 7.0));
        float y = fract((q.y + uTime * (9.0 + 4.0 * hh)) * 0.35 + hh * 13.0);
        float dash = smoothstep(0.0, 0.02, y) * smoothstep(0.10, 0.06, y) * step(hh, uRain * 0.8);
        float line = 1.0 - smoothstep(0.0, pxSize * 0.9, abs(fract(q.x / colw) - 0.5) * colw);
        col += vec3(0.55, 0.62, 0.72) * dash * line * 0.2 * (0.3 + gAmb.b * 2.0);
    }

    fragColor = vec4(col + emit, 1.0);
}
