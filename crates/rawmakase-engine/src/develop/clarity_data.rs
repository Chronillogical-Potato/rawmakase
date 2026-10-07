// Measured positive Clarity (clarity.rs): per blur scale (rows, as `SCALES`) the log2
// gain per unit of detail at base levels from `KNOT_LO` to `KNOT_HI` relative to the
// photo's highlights. Least-squares fits of the log2 change Camera Raw 18.7 renders at
// Clarity +50 and +100 on 104 synthetic scenes (docs/tone-controls.md#clarity).
const KNOTS: usize = 9;
const WEIGHTS_50: [[f32; KNOTS]; 4] = [
    [-0.0059, 0.0886, 0.0391, 0.0800, 0.1369, 0.0655, 0.1452, 0.1665, 0.1857],
    [-0.2681, 0.0884, 0.2155, -0.0102, 0.1067, 0.0977, -0.0934, -0.0171, 0.2793],
    [-0.4321, 0.4438, 0.1016, 0.2020, 0.0494, -0.0091, 0.2378, 0.4550, 0.0938],
    [1.5558, 0.0313, 0.3018, 0.3411, 0.4349, 0.4530, 0.0957, -0.3562, 0.0639],
];
const WEIGHTS_100: [[f32; KNOTS]; 4] = [
    [0.0296, 0.1427, 0.0701, 0.1435, 0.2435, 0.1028, 0.2656, 0.3351, 0.4379],
    [-0.2702, 0.1237, 0.4006, -0.0191, 0.2054, 0.1806, -0.1843, -0.0399, 0.6542],
    [-0.3154, 0.8038, 0.2576, 0.3752, 0.0892, 0.0033, 0.4552, 0.9914, 0.2460],
    [2.1135, 0.2565, 0.4892, 0.6609, 0.8496, 0.8610, 0.1903, -0.7805, 0.1187],
];
