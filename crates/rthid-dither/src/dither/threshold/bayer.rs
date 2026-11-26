//! Recursive Bayer matrices for ordered thresholds.

/// Ordered dithering matrix size.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum BayerSize {
    /// Repeats every two rows and columns.
    Two,
    /// Repeats every four rows and columns.
    #[default]
    Four,
    /// Repeats every eight rows and columns.
    Eight,
}

fn matrix(size: BayerSize) -> ([[u8; 8]; 8], usize) {
    let n = match size {
        BayerSize::Two => 2,
        BayerSize::Four => 4,
        BayerSize::Eight => 8,
    };
    let mut matrix = [[0; 8]; 8];
    let mut k = 1;
    while k < n {
        let previous = matrix;
        for y in 0..k * 2 {
            for x in 0..k * 2 {
                matrix[y][x] = previous[y % k][x % k] * 4
                    + [[0, 2], [3, 1]][usize::from(y >= k)][usize::from(x >= k)];
            }
        }
        k *= 2;
    }
    (matrix, n)
}

/// Prepares a matrix once for all pixels in an image.
pub(in crate::dither) fn threshold(size: BayerSize) -> impl Fn(usize, usize) -> f64 {
    let (matrix, n) = matrix(size);
    move |x, y| (f64::from(matrix[y % n][x % n]) + 0.5) / (n * n) as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recursive_matrices_have_expected_ranks() {
        for (size, expected) in [
            (BayerSize::Two, vec![vec![0, 2], vec![3, 1]]),
            (
                BayerSize::Four,
                vec![
                    vec![0, 8, 2, 10],
                    vec![12, 4, 14, 6],
                    vec![3, 11, 1, 9],
                    vec![15, 7, 13, 5],
                ],
            ),
        ] {
            let (matrix, n) = matrix(size);
            let rows: Vec<_> = matrix[..n].iter().map(|row| row[..n].to_vec()).collect();
            assert_eq!(rows, expected);
        }
        let (matrix, _) = matrix(BayerSize::Eight);
        let mut ranks = matrix.into_iter().flatten().collect::<Vec<_>>();
        ranks.sort_unstable();
        assert_eq!(ranks, (0..64).collect::<Vec<_>>());
    }
}
