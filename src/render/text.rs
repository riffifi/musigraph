use super::gpu::{Color, Gpu};
pub fn equation(
    g: &mut Gpu,
    x: f32,
    y: f32,
    terms: &[(f32, f32, f32, f32)],
    total: usize,
    color: Color,
) -> f32 {
    g.text(x, y, 17., "y(t) =", color);
    g.text(x, y + 30., 26., "Σ", color);
    g.text(x + 24., y + 38., 11., "k=1", color);
    g.text(x + 10., y + 13., 11., &format!("{total}"), color);
    for (i, (a, f, p, r)) in terms.iter().enumerate() {
        g.text(
            x + 65.,
            y + 12. + i as f32 * 20.,
            12.,
            &format!("{:+0.3}sin(2π·{:0.1}t{:+0.2})", a, f, p),
            color,
        );
        g.text(
            x + 268.,
            y + 12. + i as f32 * 20.,
            11.,
            &format!("{:0.1}×", r),
            color,
        );
    }
    y + 50. + terms.len() as f32 * 20.
}
