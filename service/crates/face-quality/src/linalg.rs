//! Small dense 3x3 linear algebra for the pose fit (f64).

/// Row-major 3x3 matrix.
pub type Mat3 = [[f64; 3]; 3];

pub const IDENTITY: Mat3 = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

pub fn mul(a: &Mat3, b: &Mat3) -> Mat3 {
    let mut r = [[0.0; 3]; 3];
    for (i, row) in r.iter_mut().enumerate() {
        for (j, v) in row.iter_mut().enumerate() {
            *v = (0..3).map(|k| a[i][k] * b[k][j]).sum();
        }
    }
    r
}

pub fn transpose(a: &Mat3) -> Mat3 {
    let mut r = [[0.0; 3]; 3];
    for (i, row) in r.iter_mut().enumerate() {
        for (j, v) in row.iter_mut().enumerate() {
            *v = a[j][i];
        }
    }
    r
}

pub fn det(a: &Mat3) -> f64 {
    a[0][0] * (a[1][1] * a[2][2] - a[1][2] * a[2][1])
        - a[0][1] * (a[1][0] * a[2][2] - a[1][2] * a[2][0])
        + a[0][2] * (a[1][0] * a[2][1] - a[1][1] * a[2][0])
}

pub fn apply(a: &Mat3, v: [f64; 3]) -> [f64; 3] {
    [
        a[0][0] * v[0] + a[0][1] * v[1] + a[0][2] * v[2],
        a[1][0] * v[0] + a[1][1] * v[1] + a[1][2] * v[2],
        a[2][0] * v[0] + a[2][1] * v[1] + a[2][2] * v[2],
    ]
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn norm(v: [f64; 3]) -> f64 {
    (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt()
}

/// Eigen-decomposition of a symmetric 3x3 matrix by cyclic Jacobi rotations. Returns eigenvalues
/// in descending order and the matching eigenvectors as the columns of the second matrix.
pub fn symmetric_eigen(a: &Mat3) -> ([f64; 3], Mat3) {
    let mut m = *a;
    let mut v = IDENTITY;
    for _sweep in 0..64 {
        let off = m[0][1].abs() + m[0][2].abs() + m[1][2].abs();
        let scale = m[0][0].abs() + m[1][1].abs() + m[2][2].abs();
        if off <= 1e-15 * scale.max(1e-300) {
            break;
        }
        for (p, q) in [(0usize, 1usize), (0, 2), (1, 2)] {
            if m[p][q].abs() <= 1e-300 {
                continue;
            }
            let theta = (m[q][q] - m[p][p]) / (2.0 * m[p][q]);
            let t = theta.signum() / (theta.abs() + (theta * theta + 1.0).sqrt());
            let t = if theta == 0.0 { 1.0 } else { t };
            let c = 1.0 / (t * t + 1.0).sqrt();
            let s = t * c;
            // m = J^T m J for the rotation J in the (p, q) plane.
            for row in m.iter_mut() {
                let (mkp, mkq) = (row[p], row[q]);
                row[p] = c * mkp - s * mkq;
                row[q] = s * mkp + c * mkq;
            }
            let (rp, rq) = (m[p], m[q]);
            for (k, (mpk, mqk)) in rp.iter().zip(rq.iter()).enumerate() {
                m[p][k] = c * mpk - s * mqk;
                m[q][k] = s * mpk + c * mqk;
            }
            for row in v.iter_mut() {
                let (vkp, vkq) = (row[p], row[q]);
                row[p] = c * vkp - s * vkq;
                row[q] = s * vkp + c * vkq;
            }
        }
    }
    let mut order = [0usize, 1, 2];
    order.sort_by(|&i, &j| m[j][j].total_cmp(&m[i][i]));
    let values = order.map(|i| m[i][i]);
    let mut vectors = [[0.0; 3]; 3];
    for (col, &i) in order.iter().enumerate() {
        for row in 0..3 {
            vectors[row][col] = v[row][i];
        }
    }
    (values, vectors)
}

/// Singular value decomposition `a = U diag(s) V^T` of a 3x3 matrix, singular values descending.
/// `U` is completed to a right-handed basis (its third column is `u1 x u2`), so `det(U) = +1`
/// while `det(V)` may be -1; [`nearest_rotation`] accounts for that.
pub fn svd(a: &Mat3) -> (Mat3, [f64; 3], Mat3) {
    let ata = mul(&transpose(a), a);
    let (eig, v) = symmetric_eigen(&ata);
    let s = eig.map(|e| e.max(0.0).sqrt());
    let col = |m: &Mat3, j: usize| [m[0][j], m[1][j], m[2][j]];
    let mut u_cols = [[0.0f64; 3]; 3];
    for j in 0..2 {
        let av = apply(a, col(&v, j));
        let n = norm(av);
        u_cols[j] = if n > 1e-300 {
            av.map(|x| x / n)
        } else {
            // Degenerate: any unit vector orthogonal to the previous column.
            let prev = if j == 0 { [0.0, 0.0, 1.0] } else { u_cols[0] };
            let trial = if prev[0].abs() < 0.9 {
                [1.0, 0.0, 0.0]
            } else {
                [0.0, 1.0, 0.0]
            };
            let c = cross(prev, trial);
            let cn = norm(c);
            c.map(|x| x / cn)
        };
    }
    // Re-orthogonalise the second column against the first, then complete with the cross product.
    let d = (0..3).map(|k| u_cols[1][k] * u_cols[0][k]).sum::<f64>();
    let mut u1 = [0.0; 3];
    for k in 0..3 {
        u1[k] = u_cols[1][k] - d * u_cols[0][k];
    }
    let n1 = norm(u1);
    if n1 > 1e-300 {
        u_cols[1] = u1.map(|x| x / n1);
    }
    u_cols[2] = cross(u_cols[0], u_cols[1]);
    let mut u = [[0.0; 3]; 3];
    for (j, c) in u_cols.iter().enumerate() {
        for row in 0..3 {
            u[row][j] = c[row];
        }
    }
    (u, s, v)
}

/// The rotation `R` (det +1) maximising `trace(R^T a)`: `U diag(1, 1, det(U V^T)) V^T`. This is
/// the reflection-safe optimal rotation of weighted orthogonal Procrustes (MediaPipe's
/// `ComputeOptimalRotation`). `None` when `a` is (numerically) zero.
pub fn nearest_rotation(a: &Mat3) -> Option<Mat3> {
    let frob = a.iter().flatten().map(|x| x * x).sum::<f64>().sqrt();
    if frob.is_nan() || frob <= 1e-9 {
        return None;
    }
    let (u, _s, v) = svd(a);
    let d = det(&u) * det(&v);
    let mut ud = u;
    if d < 0.0 {
        for row in ud.iter_mut() {
            row[2] = -row[2];
        }
    }
    Some(mul(&ud, &transpose(&v)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rot(axis: usize, deg: f64) -> Mat3 {
        let (c, s) = (deg.to_radians().cos(), deg.to_radians().sin());
        match axis {
            0 => [[1.0, 0.0, 0.0], [0.0, c, -s], [0.0, s, c]],
            1 => [[c, 0.0, s], [0.0, 1.0, 0.0], [-s, 0.0, c]],
            _ => [[c, -s, 0.0], [s, c, 0.0], [0.0, 0.0, 1.0]],
        }
    }

    fn close(a: &Mat3, b: &Mat3, tol: f64) -> bool {
        a.iter()
            .flatten()
            .zip(b.iter().flatten())
            .all(|(x, y)| (x - y).abs() < tol)
    }

    #[test]
    fn svd_reconstructs_and_orders_singular_values() {
        let a: Mat3 = [[2.0, -1.0, 0.5], [0.3, 4.0, 1.0], [-1.0, 0.2, 3.0]];
        let (u, s, v) = svd(&a);
        assert!(s[0] >= s[1] && s[1] >= s[2] && s[2] >= 0.0);
        let mut sigma = [[0.0; 3]; 3];
        for i in 0..3 {
            sigma[i][i] = s[i];
        }
        // det(U) = +1 by construction; if det(V) is -1 the third singular value takes its sign.
        let mut sv = sigma;
        if det(&u) * det(&v) < 0.0 {
            sv[2][2] = -s[2];
        }
        let back = mul(&mul(&u, &sv), &transpose(&v));
        assert!(close(&back, &a, 1e-9), "{back:?}");
        assert!(close(&mul(&transpose(&u), &u), &IDENTITY, 1e-9));
        assert!(close(&mul(&transpose(&v), &v), &IDENTITY, 1e-9));
    }

    #[test]
    fn nearest_rotation_recovers_rotations_and_refuses_reflections() {
        let r = mul(&mul(&rot(2, 12.0), &rot(1, -30.0)), &rot(0, 7.0));
        let scaled: Mat3 = r.map(|row| row.map(|x| 2.5 * x));
        assert!(close(&nearest_rotation(&scaled).unwrap(), &r, 1e-9));
        // A reflection's nearest rotation has det +1.
        let reflection: Mat3 = [[-1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
        let q = nearest_rotation(&reflection).unwrap();
        assert!((det(&q) - 1.0).abs() < 1e-9);
        assert!(nearest_rotation(&[[0.0; 3]; 3]).is_none());
    }

    #[test]
    fn eigen_decomposition_of_a_diagonalisable_matrix() {
        let r = rot(1, 25.0);
        let d: Mat3 = [[5.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 3.0]];
        let a = mul(&mul(&r, &d), &transpose(&r));
        let (vals, _vecs) = symmetric_eigen(&a);
        assert!(
            (vals[0] - 5.0).abs() < 1e-9
                && (vals[1] - 3.0).abs() < 1e-9
                && (vals[2] - 1.0).abs() < 1e-9
        );
    }
}
