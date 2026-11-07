// Post: bloom from the HDR mip chain, ACES filmic tone mapping, vignette, dithering.
out vec4 fragColor;
uniform sampler2D uTex;
uniform vec2 uOutRes;
uniform float uBloom;
uniform float uExposure;

vec3 aces(vec3 x) {
    const float a = 2.51, b = 0.03, c = 2.43, d = 0.59, e = 0.14;
    return clamp((x * (a * x + b)) / (x * (c * x + d) + e), 0.0, 1.0);
}

void main() {
    vec2 uv = gl_FragCoord.xy / uOutRes;
    vec3 c = texture(uTex, uv).rgb;
    if (uBloom > 0.0) {
        vec3 bl = vec3(0.0);
        bl += textureLod(uTex, uv, 1.5).rgb * 0.30;
        bl += textureLod(uTex, uv, 2.5).rgb * 0.25;
        bl += textureLod(uTex, uv, 3.5).rgb * 0.20;
        bl += textureLod(uTex, uv, 4.5).rgb * 0.15;
        bl += textureLod(uTex, uv, 5.5).rgb * 0.10;
        bl += textureLod(uTex, uv, 6.5).rgb * 0.08;
        // only the energy above ~ white glows
        bl = max(bl - vec3(0.35), vec3(0.0));
        c += bl * uBloom;
    }
    c *= uExposure;
    c = aces(c);
    vec2 v = uv - 0.5;
    c *= 1.0 - dot(v, v) * 0.55;
    c = pow(c, vec3(1.0 / 2.2));
    // blue-noise-ish dither against banding in the dark gradients
    float n = fract(sin(dot(gl_FragCoord.xy, vec2(12.9898, 78.233))) * 43758.5453);
    c += (n - 0.5) / 255.0;
    fragColor = vec4(c, 1.0);
}
