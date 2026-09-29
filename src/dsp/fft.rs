use std::f32::consts::PI;

pub fn complex(re: &mut [f32], im: &mut [f32], inverse: bool) {
    let n = re.len();
    assert!(n.is_power_of_two() && im.len() == n);
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j ^= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        let angle = (if inverse { 2.0 } else { -2.0 }) * PI / len as f32;
        let (wi, wr) = angle.sin_cos();
        for base in (0..n).step_by(len) {
            let (mut ur, mut ui) = (1.0, 0.0);
            for k in 0..len / 2 {
                let a = base + k;
                let b = a + len / 2;
                let vr = re[b] * ur - im[b] * ui;
                let vi = re[b] * ui + im[b] * ur;
                re[b] = re[a] - vr;
                im[b] = im[a] - vi;
                re[a] += vr;
                im[a] += vi;
                let next = ur * wr - ui * wi;
                ui = ur * wi + ui * wr;
                ur = next;
            }
        }
        len <<= 1;
    }
    if inverse {
        for x in re.iter_mut() {
            *x /= n as f32;
        }
        for x in im.iter_mut() {
            *x /= n as f32;
        }
    }
}

pub fn real(input: &[f32]) -> (Vec<f32>, Vec<f32>) {
    let n = input.len();
    assert!(n.is_power_of_two());
    let h = n / 2;
    let mut re = vec![0.0; h];
    let mut im = vec![0.0; h];
    for i in 0..h {
        re[i] = input[2 * i];
        im[i] = input[2 * i + 1];
    }
    complex(&mut re, &mut im, false);
    let mut out_re = vec![0.0; h + 1];
    let mut out_im = vec![0.0; h + 1];
    out_re[0] = re[0] + im[0];
    out_re[h] = re[0] - im[0];
    for k in 1..h {
        let a_re = re[k];
        let a_im = im[k];
        let b_re = re[h - k];
        let b_im = -im[h - k];
        let er = (a_re + b_re) * 0.5;
        let ei = (a_im + b_im) * 0.5;
        let or = (a_im - b_im) * 0.5;
        let oi = -(a_re - b_re) * 0.5;
        let angle = -2.0 * PI * k as f32 / n as f32;
        let (s, c) = angle.sin_cos();
        out_re[k] = er + c * or - s * oi;
        out_im[k] = ei + s * or + c * oi;
    }
    (out_re, out_im)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn matches_dft() {
        let v: Vec<f32> = (0..32)
            .map(|i| (i as f32 * 0.37).sin() + 0.2 * (i as f32 * 0.9).cos())
            .collect();
        let (re, im) = real(&v);
        for k in 0..=16 {
            let (mut dr, mut di) = (0., 0.);
            for (j, &x) in v.iter().enumerate() {
                let a = -2. * PI * k as f32 * j as f32 / 32.;
                dr += x * a.cos();
                di += x * a.sin();
            }
            assert!((dr - re[k]).abs() < 1e-4, "re {k}");
            assert!((di - im[k]).abs() < 1e-4, "im {k}");
        }
    }
}
