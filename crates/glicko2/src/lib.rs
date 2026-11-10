//! A small, dependency-free implementation of the Glicko-2 rating system,
//! with defaults modelled on lichess.
//!
//! See Mark Glickman, "Example of the Glicko-2 system".

use std::f64::consts::PI;

/// Conversion factor between the Glicko and Glicko-2 scales.
const SCALE: f64 = 173.7178;
/// Convergence tolerance of the volatility iteration.
const EPSILON: f64 = 1e-6;
/// Center of the rating scale.
const BASE_RATING: f64 = 1500.0;

/// A player's rating state on the Glicko (not Glicko-2) display scale.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rating {
    /// Rating `r`; 1500 is average.
    pub rating: f64,
    /// Rating deviation `RD`; the uncertainty of the rating.
    pub deviation: f64,
    /// Volatility `sigma`; expected fluctuation of the rating.
    pub volatility: f64,
}

impl Rating {
    /// Creates a rating from its three components.
    pub fn new(rating: f64, deviation: f64, volatility: f64) -> Self {
        Self { rating, deviation, volatility }
    }
}

impl Default for Rating {
    /// 1500 / 350 / 0.06.
    fn default() -> Self {
        Self::new(1500.0, 350.0, 0.06)
    }
}

/// Result of a game from the point of view of one player.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// The player won.
    Win,
    /// The game was drawn.
    Draw,
    /// The player lost.
    Loss,
}

impl Outcome {
    /// Score of the outcome: 1.0 for a win, 0.5 for a draw, 0.0 for a loss.
    pub fn score(self) -> f64 {
        match self {
            Outcome::Win => 1.0,
            Outcome::Draw => 0.5,
            Outcome::Loss => 0.0,
        }
    }

    fn flip(self) -> Self {
        match self {
            Outcome::Win => Outcome::Loss,
            Outcome::Draw => Outcome::Draw,
            Outcome::Loss => Outcome::Win,
        }
    }
}

/// Tunable parameters of the system.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Config {
    /// System constant `tau` constraining volatility change (lichess: 0.75).
    pub tau: f64,
    /// Lower bound for the rating deviation after an update (lichess: 45).
    pub min_deviation: f64,
    /// Upper bound for the rating deviation after an update (lichess: 500).
    pub max_deviation: f64,
}

impl Default for Config {
    fn default() -> Self {
        Self { tau: 0.75, min_deviation: 45.0, max_deviation: 500.0 }
    }
}

fn g(phi: f64) -> f64 {
    1.0 / (1.0 + 3.0 * phi * phi / (PI * PI)).sqrt()
}

fn e(mu: f64, mu_j: f64, phi_j: f64) -> f64 {
    1.0 / (1.0 + (-g(phi_j) * (mu - mu_j)).exp())
}

/// Expected score of `a` against `b` (between 0 and 1), taking `b`'s
/// deviation into account. Note this is not exactly symmetric in general
/// (`expected_score(a, b) + expected_score(b, a)` equals 1 only when both
/// deviations are equal).
pub fn expected_score(a: Rating, b: Rating) -> f64 {
    let mu = (a.rating - BASE_RATING) / SCALE;
    let mu_j = (b.rating - BASE_RATING) / SCALE;
    e(mu, mu_j, b.deviation / SCALE)
}

/// Whether the rating is provisional (deviation > 110); lichess shows "?".
pub fn provisional(r: &Rating) -> bool {
    r.deviation > 110.0
}

/// Updates `player` after one rating period containing `games`
/// (opponent rating at the time of the game and outcome for `player`).
///
/// With no games only the deviation grows. The resulting deviation is
/// clamped to `[cfg.min_deviation, cfg.max_deviation]`.
pub fn update(player: Rating, games: &[(Rating, Outcome)], cfg: &Config) -> Rating {
    let mu = (player.rating - BASE_RATING) / SCALE;
    let phi = player.deviation / SCALE;
    let sigma = player.volatility;

    let (mu_new, phi_new, sigma_new) = if games.is_empty() {
        (mu, (phi * phi + sigma * sigma).sqrt(), sigma)
    } else {
        // Step 3: estimated variance v, and step 4: improvement delta.
        let mut v_inv = 0.0;
        let mut delta_sum = 0.0;
        for (opp, outcome) in games {
            let mu_j = (opp.rating - BASE_RATING) / SCALE;
            let phi_j = opp.deviation / SCALE;
            let gj = g(phi_j);
            let ej = e(mu, mu_j, phi_j);
            v_inv += gj * gj * ej * (1.0 - ej);
            delta_sum += gj * (outcome.score() - ej);
        }
        let v = 1.0 / v_inv;
        let delta = v * delta_sum;

        // Step 5: new volatility via the Illinois algorithm.
        let sigma_new = new_volatility(phi, sigma, v, delta, cfg.tau);

        // Steps 6-7.
        let phi_star = (phi * phi + sigma_new * sigma_new).sqrt();
        let phi_new = 1.0 / (1.0 / (phi_star * phi_star) + 1.0 / v).sqrt();
        let mu_new = mu + phi_new * phi_new * delta_sum;
        (mu_new, phi_new, sigma_new)
    };

    Rating {
        rating: mu_new * SCALE + BASE_RATING,
        deviation: (phi_new * SCALE).clamp(cfg.min_deviation, cfg.max_deviation),
        volatility: sigma_new,
    }
}

fn new_volatility(phi: f64, sigma: f64, v: f64, delta: f64, tau: f64) -> f64 {
    let a = (sigma * sigma).ln();
    let f = |x: f64| {
        let ex = x.exp();
        let d2 = phi * phi + v + ex;
        ex * (delta * delta - phi * phi - v - ex) / (2.0 * d2 * d2) - (x - a) / (tau * tau)
    };

    let mut big_a = a;
    let mut big_b = if delta * delta > phi * phi + v {
        (delta * delta - phi * phi - v).ln()
    } else {
        let mut k = 1.0;
        while f(a - k * tau) < 0.0 {
            k += 1.0;
            if k > 1000.0 {
                break;
            }
        }
        a - k * tau
    };

    let mut fa = f(big_a);
    let mut fb = f(big_b);
    let mut iterations = 0;
    while (big_b - big_a).abs() > EPSILON && iterations < 1000 {
        let c = big_a + (big_a - big_b) * fa / (fb - fa);
        let fc = f(c);
        if fc * fb <= 0.0 {
            big_a = big_b;
            fa = fb;
        } else {
            fa /= 2.0;
        }
        big_b = c;
        fb = fc;
        iterations += 1;
    }
    (big_a / 2.0).exp()
}

/// Updates both players after a single game, each treating the game as its
/// own rating period (lichess style). Returns `(white, black)`.
pub fn update_game(
    white: Rating,
    black: Rating,
    white_outcome: Outcome,
    cfg: &Config,
) -> (Rating, Rating) {
    (
        update(white, &[(black, white_outcome)], cfg),
        update(black, &[(white, white_outcome.flip())], cfg),
    )
}

/// Game speed category, using the lichess clock estimate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Speed {
    /// Estimated duration under 30 seconds.
    UltraBullet,
    /// Under 180 seconds.
    Bullet,
    /// Under 480 seconds.
    Blitz,
    /// Under 1500 seconds.
    Rapid,
    /// 1500 seconds or more.
    Classical,
}

impl Speed {
    /// Classifies a clock by `initial_secs + 40 * increment_secs`.
    /// Both arguments are in seconds (a "3+2" game is `from_clock(180, 2)`).
    pub fn from_clock(initial_secs: u32, increment_secs: u32) -> Speed {
        let estimate = u64::from(initial_secs) + 40 * u64::from(increment_secs);
        match estimate {
            0..30 => Speed::UltraBullet,
            30..180 => Speed::Bullet,
            180..480 => Speed::Blitz,
            480..1500 => Speed::Rapid,
            _ => Speed::Classical,
        }
    }

    /// Lichess-style name, e.g. `"bullet"` (`"ultraBullet"` for ultra bullet).
    pub fn name(&self) -> &'static str {
        match self {
            Speed::UltraBullet => "ultraBullet",
            Speed::Bullet => "bullet",
            Speed::Blitz => "blitz",
            Speed::Rapid => "rapid",
            Speed::Classical => "classical",
        }
    }

    /// All speeds, fastest first.
    pub fn all() -> [Speed; 5] {
        [Speed::UltraBullet, Speed::Bullet, Speed::Blitz, Speed::Rapid, Speed::Classical]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() < tol
    }

    #[test]
    fn glickman_worked_example() {
        let cfg = Config { tau: 0.5, min_deviation: 0.0, max_deviation: 1000.0 };
        let p = Rating::new(1500.0, 200.0, 0.06);
        let games = [
            (Rating::new(1400.0, 30.0, 0.06), Outcome::Win),
            (Rating::new(1550.0, 100.0, 0.06), Outcome::Loss),
            (Rating::new(1700.0, 300.0, 0.06), Outcome::Loss),
        ];
        let r = update(p, &games, &cfg);
        assert!(close(r.rating, 1464.06, 0.01), "rating {}", r.rating);
        assert!(close(r.deviation, 151.52, 0.01), "rd {}", r.deviation);
        assert!(close(r.volatility, 0.05999, 0.00001), "vol {}", r.volatility);
    }

    #[test]
    fn no_games_grows_deviation() {
        let cfg = Config::default();
        let p = Rating::new(1600.0, 100.0, 0.06);
        let r = update(p, &[], &cfg);
        let phi = 100.0 / SCALE;
        let expected = (phi * phi + 0.06f64 * 0.06).sqrt() * SCALE;
        assert!(close(r.deviation, expected, 1e-9));
        assert_eq!(r.rating, 1600.0);
        assert_eq!(r.volatility, 0.06);
    }

    #[test]
    fn deviation_is_clamped() {
        let cfg = Config::default();
        let r = update(Rating::new(1500.0, 499.9, 0.5), &[], &cfg);
        assert_eq!(r.deviation, 500.0);
        let r = update(Rating::new(1500.0, 10.0, 0.0001), &[], &cfg);
        assert_eq!(r.deviation, 45.0);
    }

    #[test]
    fn equal_players_draw_is_symmetric() {
        let cfg = Config::default();
        let p = Rating::default();
        let (w, b) = update_game(p, p, Outcome::Draw, &cfg);
        assert!(close(w.rating, 1500.0, 1e-9));
        assert!(close(b.rating, 1500.0, 1e-9));
        assert!(close(w.deviation, b.deviation, 1e-9));
        assert!(w.deviation < 350.0);
    }

    #[test]
    fn equal_players_win_is_antisymmetric() {
        let cfg = Config::default();
        let p = Rating::default();
        let (w, b) = update_game(p, p, Outcome::Win, &cfg);
        assert!(close(w.rating - 1500.0, 1500.0 - b.rating, 1e-9));
        assert!(w.rating > 1500.0);
    }

    #[test]
    fn from_clock_boundaries() {
        // Arguments are seconds: 1+0 minutes is 60s.
        assert_eq!(Speed::from_clock(15, 0), Speed::UltraBullet);
        assert_eq!(Speed::from_clock(1, 0), Speed::UltraBullet);
        assert_eq!(Speed::from_clock(60, 0), Speed::Bullet); // 1+0
        assert_eq!(Speed::from_clock(180, 2), Speed::Blitz); // 3+2 -> 260
        assert_eq!(Speed::from_clock(600, 0), Speed::Rapid); // 10+0
        assert_eq!(Speed::from_clock(900, 10), Speed::Rapid); // 15+10 -> 1300
        assert_eq!(Speed::from_clock(1800, 0), Speed::Classical); // 30+0
        assert_eq!(Speed::from_clock(179, 0), Speed::Bullet);
        assert_eq!(Speed::from_clock(180, 0), Speed::Blitz);
        assert_eq!(Speed::from_clock(479, 0), Speed::Blitz);
        assert_eq!(Speed::from_clock(480, 0), Speed::Rapid);
        assert_eq!(Speed::from_clock(1499, 0), Speed::Rapid);
        assert_eq!(Speed::from_clock(1500, 0), Speed::Classical);
        assert_eq!(Speed::from_clock(0, 1), Speed::Bullet); // 40
    }

    #[test]
    fn speed_names_and_all() {
        assert_eq!(Speed::Bullet.name(), "bullet");
        assert_eq!(Speed::all().len(), 5);
        assert_eq!(Speed::all()[4], Speed::Classical);
    }

    #[test]
    fn expected_score_symmetric() {
        let a = Rating::new(1700.0, 80.0, 0.06);
        let b = Rating::new(1500.0, 80.0, 0.06);
        assert!(close(expected_score(a, b) + expected_score(b, a), 1.0, 1e-12));
        assert!(expected_score(a, b) > 0.5);
        assert!(close(expected_score(a, a), 0.5, 1e-12));
    }

    #[test]
    fn provisional_threshold() {
        assert!(provisional(&Rating::default()));
        assert!(!provisional(&Rating::new(1500.0, 110.0, 0.06)));
    }
}
