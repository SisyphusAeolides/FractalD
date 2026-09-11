use std::f64::consts::PI;

unsafe extern "C" {
    fn fractald_logistic_step(parameter: f64, value: f64) -> f64;
    fn fractald_mandelbrot_escape(
        real: f64,
        imaginary: f64,
        maximum_iterations: u32,
        escape_radius_squared: f64,
    ) -> u32;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SystemKind {
    Lorenz,
    Mandelbrot,
    Lyapunov,
    Rossler,
    LogisticMap,
    Duffing,
}

impl SystemKind {
    pub const ALL: [Self; 6] = [
        Self::Lorenz,
        Self::Mandelbrot,
        Self::Lyapunov,
        Self::Rossler,
        Self::LogisticMap,
        Self::Duffing,
    ];

    pub const fn name(self) -> &'static str {
        match self {
            Self::Lorenz => "lorenz",
            Self::Mandelbrot => "mandelbrot",
            Self::Lyapunov => "lyapunov",
            Self::Rossler => "rossler",
            Self::LogisticMap => "logistic-map",
            Self::Duffing => "duffing",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.name() == name)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Vec3 {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

impl Vec3 {
    pub const fn new(x: f64, y: f64, z: f64) -> Self {
        Self { x, y, z }
    }

    fn scaled_add(self, other: Self, factor: f64) -> Self {
        Self {
            x: self.x + other.x * factor,
            y: self.y + other.y * factor,
            z: self.z + other.z * factor,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Lorenz {
    pub sigma: f64,
    pub rho: f64,
    pub beta: f64,
}

impl Default for Lorenz {
    fn default() -> Self {
        Self {
            sigma: 10.0,
            rho: 28.0,
            beta: 8.0 / 3.0,
        }
    }
}

impl Lorenz {
    pub fn derivative(&self, state: Vec3) -> Vec3 {
        Vec3::new(
            self.sigma * (state.y - state.x),
            state.x * (self.rho - state.z) - state.y,
            state.x * state.y - self.beta * state.z,
        )
    }

    pub fn step(&self, state: Vec3, dt: f64) -> Vec3 {
        rk4_vec3(state, dt, |value| self.derivative(value))
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rossler {
    pub a: f64,
    pub b: f64,
    pub c: f64,
}

impl Default for Rossler {
    fn default() -> Self {
        Self {
            a: 0.2,
            b: 0.2,
            c: 5.7,
        }
    }
}

impl Rossler {
    pub fn derivative(&self, state: Vec3) -> Vec3 {
        Vec3::new(
            -state.y - state.z,
            state.x + self.a * state.y,
            self.b + state.z * (state.x - self.c),
        )
    }

    pub fn step(&self, state: Vec3, dt: f64) -> Vec3 {
        rk4_vec3(state, dt, |value| self.derivative(value))
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DuffingState {
    pub position: f64,
    pub velocity: f64,
}

impl DuffingState {
    pub const fn new(position: f64, velocity: f64) -> Self {
        Self { position, velocity }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Duffing {
    pub alpha: f64,
    pub beta: f64,
    pub damping: f64,
    pub drive_amplitude: f64,
    pub drive_frequency: f64,
}

impl Default for Duffing {
    fn default() -> Self {
        Self {
            alpha: -1.0,
            beta: 1.0,
            damping: 0.2,
            drive_amplitude: 0.3,
            drive_frequency: 1.2,
        }
    }
}

impl Duffing {
    pub fn derivative(&self, state: DuffingState, time: f64) -> DuffingState {
        DuffingState::new(
            state.velocity,
            self.drive_amplitude * (self.drive_frequency * time).cos()
                - self.damping * state.velocity
                - self.alpha * state.position
                - self.beta * state.position.powi(3),
        )
    }

    pub fn step(&self, state: DuffingState, time: f64, dt: f64) -> DuffingState {
        let k1 = self.derivative(state, time);
        let k2 = self.derivative(state.add_scaled(k1, dt / 2.0), time + dt / 2.0);
        let k3 = self.derivative(state.add_scaled(k2, dt / 2.0), time + dt / 2.0);
        let k4 = self.derivative(state.add_scaled(k3, dt), time + dt);
        DuffingState::new(
            state.position
                + dt * (k1.position + 2.0 * k2.position + 2.0 * k3.position + k4.position) / 6.0,
            state.velocity
                + dt * (k1.velocity + 2.0 * k2.velocity + 2.0 * k3.velocity + k4.velocity) / 6.0,
        )
    }
}

impl DuffingState {
    fn add_scaled(self, other: Self, factor: f64) -> Self {
        Self::new(
            self.position + other.position * factor,
            self.velocity + other.velocity * factor,
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LogisticMap {
    pub parameter: f64,
}

impl LogisticMap {
    pub const fn new(parameter: f64) -> Self {
        Self { parameter }
    }

    pub fn next(self, value: f64) -> f64 {
        unsafe { fractald_logistic_step(self.parameter, value) }
    }

    pub fn sequence(self, initial: f64, count: usize) -> Vec<f64> {
        let mut values = Vec::with_capacity(count);
        let mut value = initial;
        for _ in 0..count {
            value = self.next(value);
            values.push(value);
        }
        values
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mandelbrot {
    pub maximum_iterations: u32,
    pub escape_radius_squared: f64,
}

impl Default for Mandelbrot {
    fn default() -> Self {
        Self {
            maximum_iterations: 256,
            escape_radius_squared: 4.0,
        }
    }
}

impl Mandelbrot {
    pub const fn new(maximum_iterations: u32) -> Self {
        Self {
            maximum_iterations,
            escape_radius_squared: 4.0,
        }
    }

    pub fn escape_iterations(self, real: f64, imaginary: f64) -> u32 {
        unsafe {
            fractald_mandelbrot_escape(
                real,
                imaginary,
                self.maximum_iterations,
                self.escape_radius_squared,
            )
        }
    }

    pub fn is_inside(self, real: f64, imaginary: f64) -> bool {
        self.escape_iterations(real, imaginary) == self.maximum_iterations
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Lyapunov;

impl Lyapunov {
    pub fn logistic(map: LogisticMap, initial: f64, transient: usize, iterations: usize) -> f64 {
        if iterations == 0 {
            return f64::NAN;
        }
        let mut value = initial;
        for _ in 0..transient {
            value = map.next(value);
        }
        let mut total = 0.0;
        for _ in 0..iterations {
            let derivative = (map.parameter * (1.0 - 2.0 * value)).abs();
            total += derivative.max(f64::MIN_POSITIVE).ln();
            value = map.next(value);
        }
        total / iterations as f64
    }

    pub fn from_series(series: &[f64]) -> f64 {
        if series.len() < 2 {
            return f64::NAN;
        }
        let mut total = 0.0;
        let mut count = 0;
        for pair in series.windows(2) {
            let distance = (pair[1] - pair[0]).abs();
            if distance.is_normal() {
                total += distance.ln();
                count += 1;
            }
        }
        if count == 0 {
            f64::NEG_INFINITY
        } else {
            total / count as f64
        }
    }
}

fn rk4_vec3<F>(state: Vec3, dt: f64, derivative: F) -> Vec3
where
    F: Fn(Vec3) -> Vec3,
{
    let k1 = derivative(state);
    let k2 = derivative(state.scaled_add(k1, dt / 2.0));
    let k3 = derivative(state.scaled_add(k2, dt / 2.0));
    let k4 = derivative(state.scaled_add(k3, dt));
    state.scaled_add(
        Vec3::new(
            k1.x + 2.0 * k2.x + 2.0 * k3.x + k4.x,
            k1.y + 2.0 * k2.y + 2.0 * k3.y + k4.y,
            k1.z + 2.0 * k2.z + 2.0 * k3.z + k4.z,
        ),
        dt / 6.0,
    )
}

pub fn phase(time: f64, frequency: f64) -> f64 {
    (time * frequency).rem_euclid(2.0 * PI)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_systems_are_named_and_parseable() {
        for kind in SystemKind::ALL {
            assert_eq!(SystemKind::parse(kind.name()), Some(kind));
        }
    }

    #[test]
    fn lorenz_derivative_matches_reference_point() {
        let derivative = Lorenz::default().derivative(Vec3::new(1.0, 1.0, 1.0));
        assert_eq!(derivative.x, 0.0);
        assert_eq!(derivative.y, 26.0);
        assert!((derivative.z + 5.0 / 3.0).abs() < 1e-12);
    }

    #[test]
    fn rossler_derivative_matches_reference_point() {
        let derivative = Rossler::default().derivative(Vec3::new(1.0, 2.0, 3.0));
        assert_eq!(derivative.x, -5.0);
        assert!((derivative.y - 1.4).abs() < 1e-12);
        assert!((derivative.z + 13.9).abs() < 1e-12);
    }

    #[test]
    fn duffing_step_remains_finite() {
        let oscillator = Duffing::default();
        let state = oscillator.step(DuffingState::new(0.1, 0.0), 0.0, 0.01);
        assert!(state.position.is_finite());
        assert!(state.velocity.is_finite());
    }

    #[test]
    fn logistic_map_uses_the_c_kernel() {
        assert_eq!(LogisticMap::new(4.0).next(0.5), 1.0);
    }

    #[test]
    fn mandelbrot_classifies_inside_and_escape_points() {
        let set = Mandelbrot::new(64);
        assert!(set.is_inside(0.0, 0.0));
        assert!(!set.is_inside(2.0, 0.0));
        assert!(set.escape_iterations(2.0, 0.0) < 64);
    }

    #[test]
    fn logistic_lyapunov_at_four_is_ln_two() {
        let exponent = Lyapunov::logistic(LogisticMap::new(4.0), 0.2, 1_000, 20_000);
        assert!((exponent - 2.0_f64.ln()).abs() < 0.01, "{exponent}");
    }

    #[test]
    fn lyapunov_handles_short_series() {
        assert!(Lyapunov::from_series(&[1.0]).is_nan());
        assert_eq!(Lyapunov::from_series(&[1.0, 1.0]), f64::NEG_INFINITY);
    }
}
