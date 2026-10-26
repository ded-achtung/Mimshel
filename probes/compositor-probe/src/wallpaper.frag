precision highp float;
varying vec2 v_coords;
uniform vec2 size;
uniform float alpha;
uniform float time;
#ifdef DEBUG_FLAGS
uniform float tint;
#endif
float hash(vec2 p) {
    return fract(sin(dot(p, vec2(127.1, 311.7))) * 43758.5453);
}
void main() {
    vec2 uv = v_coords;
    vec2 p = (uv - 0.5) * vec2(size.x / size.y, 1.0);
    float cloud = sin(p.x * 3.0 + time * 0.07 + sin(p.y * 4.0))
                * cos(p.y * 3.8 - time * 0.04 + sin(p.x * 2.0));
    vec3 color = mix(vec3(0.012, 0.022, 0.075), vec3(0.12, 0.045, 0.21), cloud * 0.5 + 0.5);
    vec2 stars = (p + vec2(time * 0.002, time * 0.0008)) * 95.0;
    vec2 cell = floor(stars);
    vec2 center = vec2(hash(cell), hash(cell + 73.0)) * 0.7 + 0.15;
    float star = 1.0 - smoothstep(0.015, 0.10, length(fract(stars) - center));
    star *= step(0.96, hash(cell + 17.0));
    color += vec3(0.65, 0.75, 1.0) * star * (0.65 + 0.35 * sin(time * 0.5 + hash(cell) * 6.28));
#ifdef DEBUG_FLAGS
    color = mix(color, vec3(0.0, 1.0, 0.0), tint * 0.2);
#endif
    gl_FragColor = vec4(color * alpha, alpha);
}
