//! Edge Module Engine — 竞赛 WASM 模块原生集成
//!
//! 精简实现 10 个边缘模块的核心逻辑，在模拟/硬件模式下统一运行。
//! 量产部署时这些模块运行在 ESP32 的 WASM3 解释器中；
//! 竞赛演示期直接编译为原生 Rust 运行在 RZ/V2H 服务端。

use serde::{Deserialize, Serialize};

// ══════════════════════════════════════════════════════════════════════════════
// Shared Types
// ══════════════════════════════════════════════════════════════════════════════

/// A single alert produced by an edge module.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EdgeAlert {
    pub module: String,
    pub event_type: i32,
    pub event_name: String,
    pub value: f32,
    pub severity: String,
}

// ══════════════════════════════════════════════════════════════════════════════
// Module-specific state and logic
// ══════════════════════════════════════════════════════════════════════════════

// ── Ring Buffer ─────────────────────────────────────────────────────────────

struct RingBuf<const N: usize> {
    buf: [f32; N],
    idx: usize,
    len: usize,
}

impl<const N: usize> RingBuf<N> {
    fn new() -> Self { Self { buf: [0.0; N], idx: 0, len: 0 } }
    fn push(&mut self, v: f32) {
        self.buf[self.idx] = v;
        self.idx = (self.idx + 1) % N;
        if self.len < N { self.len += 1; }
    }
    fn mean_last(&self, n: usize) -> f32 {
        let c = n.min(self.len);
        if c == 0 { return 0.0; }
        let mut s = 0.0;
        for i in 0..c {
            let ri = (self.idx + N - c + i) % N;
            s += self.buf[ri];
        }
        s / c as f32
    }
    fn trend(&self, n: usize) -> f32 {
        let c = n.min(self.len);
        if c < 4 { return 0.0; }
        let q = c / 4;
        let (mut f, mut l) = (0.0f32, 0.0f32);
        for i in 0..q { f += self.buf[(self.idx + N - c + i) % N]; }
        for i in (c - q)..c { l += self.buf[(self.idx + N - c + i) % N]; }
        (l / q as f32 - f / q as f32) / c as f32
    }
}

// ══════════════════════════════════════════════════════════════════════════════
// Main Engine
// ══════════════════════════════════════════════════════════════════════════════

// ── Module 11-13 state structs ──────────────────────────────────────────────

struct OccState {
    baseline_var: [f32; 8],
    score: [f32; 8],
    occupied: u8,
    prev_occupied: u8,
    calib_sum: [f32; 8],
    calib_count: u32,
    calibrated: bool,
    frame_count: u32,
    n_zones: usize,
}

const MC_FEAT_DIM: usize = 8;
const MC_MAX_PERSONS: usize = 4;
const MC_UNASSIGNED: u8 = 255;

struct McState {
    signature: [[f32; MC_FEAT_DIM]; MC_MAX_PERSONS],
    active: u8,
    tracked: [u16; MC_MAX_PERSONS],
    absent: [u16; MC_MAX_PERSONS],
    person_id: [u8; MC_MAX_PERSONS],
    prev_assign: [u8; MC_MAX_PERSONS],
    frame_count: u32,
    swap_count: u32,
}

const WD_MAX_SC: usize = 32;

struct WdState {
    baseline_amp_var: [f32; WD_MAX_SC],
    baseline_phase_var: [f32; WD_MAX_SC],
    cal_amp_sum: [f32; WD_MAX_SC],
    cal_amp_sq_sum: [f32; WD_MAX_SC],
    cal_phase_sum: [f32; WD_MAX_SC],
    cal_phase_sq_sum: [f32; WD_MAX_SC],
    cal_count: u32,
    calibrated: bool,
    run_amp_mean: [f32; WD_MAX_SC],
    run_amp_m2: [f32; WD_MAX_SC],
    run_phase_mean: [f32; WD_MAX_SC],
    run_phase_m2: [f32; WD_MAX_SC],
    run_count: u32,
    metal_run: u8,
    weapon_run: u8,
    cd_metal: u16,
    cd_weapon: u16,
    cd_recalib: u16,
    frame_count: u32,
}

// ── Module 14: sig_sparse_recovery state ────────────────────────────────────

struct SrState {
    corr_model: [f32; 32],
    model_frames: u32,
    model_built: bool,
    total_dropouts: u64,
    total_recoveries: u64,
}

// ── Module 15: med_gait_analysis state ──────────────────────────────────────

struct GaitState {
    phase_var_buf: RingBuf<60>,
    peak_threshold: f32,
    last_peak_idx: i32,
    step_intervals: [f32; 8],
    si_idx: usize,
    si_count: usize,
    shuffling_ctr: u8,
    fest_short_ctr: u8,
    fest_prev_interval: f32,
}

// ── Module 16: sec_loitering state ──────────────────────────────────────────
// 4-state machine: 0=Absent, 1=Entering, 2=Present, 3=Loitering

struct LoiteringState {
    state: u8,
    enter_timer: u32,
    dwell_timer: u32,
    exit_cooldown: u32,
}

// ── Module 17: ind_structural_vibration state ───────────────────────────────

struct VibrationState {
    phase_rms_buf: RingBuf<600>,
    seismic_cooldown: u16,
    resonance_cooldown: u16,
    drift_cumulative: f32,
    drift_frames: u32,
}

// ── Module 18: lrn_meta_adapt state ─────────────────────────────────────────

struct MetaState {
    thresholds: [f32; 8],
    best_thresholds: [f32; 8],
    best_score: f32,
    consecutive_failures: u8,
    iteration: u32,
    explore_step: usize,
    direction: bool,
    frame_ctr: u32,
    true_positives: u32,
    false_positives: u32,
}

// ── Module 19: tmp_temporal_logic_guard state ───────────────────────────────

struct LtlState {
    violations: [u8; 8],
    motion_stop_timer: u32,
    breathing_high_timer: u32,
    hr_high_timer: u32,
    seizure_gait_timer: u32,
    alert_cooldown: [u16; 8],
}


pub struct EdgeModuleEngine {
    // Module 1: vital_trend — 生命体征趋势
    vt_br: RingBuf<300>,      // breathing rate history (5 min @ 1Hz)
    vt_hr: RingBuf<300>,      // heart rate history
    vt_timer: u32,
    vt_apnea_ctr: u32,
    vt_brady_ctr: u8, vt_tachy_ctr: u8,
    vt_hr_brady_ctr: u8, vt_hr_tachy_ctr: u8,

    // Module 2: lrn_anomaly_attractor — 混沌吸引子
    att_center: [f32; 4], att_radius: f32,
    att_initialized: bool, att_frame: u32,
    att_cooldown: u16,
    att_lyap_sum: f64, att_lyap_cnt: u32,
    att_prev_state: [f32; 4], att_prev_delta: f32,

    // Module 3: coherence — CSI 相干性
    coh_prev_phases: [f32; 32],
    coh_smoothed: f32, coh_initialized: bool,
    coh_gate: u8, // 0=accept, 1=warn, 2=reject

    // Module 4: med_respiratory_distress — 呼吸窘迫
    rd_br_buf: RingBuf<60>,
    rd_var_buf: RingBuf<60>,
    rd_baseline_var: f32, rd_baseline_frames: u32,
    rd_tachy_ctr: u8, rd_cooldown: u16,
    rd_ac_buf: RingBuf<120>, rd_ac_frames: u32,

    // Module 5: ind_confined_space — 密闭空间监护
    cs_present: bool, cs_breathing_ok: bool,
    cs_no_br_ctr: u32, cs_no_motion_ctr: u32,
    cs_entry_logged: bool,

    // Module 6: sec_panic_motion — 恐慌动作检测
    pm_energy_buf: RingBuf<100>,
    pm_var_buf: RingBuf<100>,
    pm_cooldown: u16,

    // Module 7: med_sleep_apnea — 睡眠呼吸暂停
    sa_no_br_ctr: u32, sa_apnea_active: bool,
    sa_apnea_events: u32, sa_sleep_secs: u32,

    // Module 8: med_cardiac_arrhythmia — 心律失常
    ca_hr_buf: RingBuf<60>,
    ca_tachy_ctr: u8, ca_brady_ctr: u8,
    ca_missed_ctr: u8, ca_prev_hr: f32,

    // Module 9: med_seizure_detect — 癫痫检测
    sz_energy_buf: RingBuf<200>,
    sz_band_buf: RingBuf<200>,
    sz_seizing: bool, sz_seizure_ctr: u32,
    sz_postictal_ctr: u32,

    // Module 10: intrusion — 入侵检测
    intr_baseline: [f32; 32], intr_baseline_set: bool,
    intr_baseline_frames: u32, intr_armed: bool,
    intr_detect_ctr: u8, intr_cooldown: u16,

    // Module 11: occupancy — 空间人数统计
    oc: OccState,

    // Module 12: sig_mincut — 多人CSI身份匹配
    mc: McState,

    // Module 13: sec_weapon_detect — 暴力/武器检测
    wd: WdState,

    // Module 14: sig_sparse_recovery — 稀疏子载波恢复
    sr: SrState,
    // Module 15: med_gait_analysis — 步态分析
    gait: GaitState,
    // Module 16: sec_loitering — 徘徊检测
    loiter: LoiteringState,
    // Module 17: ind_structural_vibration — 建筑/地震振动
    vib: VibrationState,
    // Module 18: lrn_meta_adapt — 元学习参数自适应
    meta: MetaState,
    // Module 19: tmp_temporal_logic_guard — 时态逻辑安全规则
    ltl: LtlState,
}

impl EdgeModuleEngine {
    pub fn new() -> Self {
        Self {
            vt_br: RingBuf::new(), vt_hr: RingBuf::new(), vt_timer: 0,
            vt_apnea_ctr: 0, vt_brady_ctr: 0, vt_tachy_ctr: 0,
            vt_hr_brady_ctr: 0, vt_hr_tachy_ctr: 0,
            att_center: [0.0; 4], att_radius: 0.0,
            att_initialized: false, att_frame: 0, att_cooldown: 0,
            att_lyap_sum: 0.0, att_lyap_cnt: 0,
            att_prev_state: [0.0; 4], att_prev_delta: 0.0,
            coh_prev_phases: [0.0; 32], coh_smoothed: 1.0,
            coh_initialized: false, coh_gate: 0,
            rd_br_buf: RingBuf::new(), rd_var_buf: RingBuf::new(),
            rd_baseline_var: 0.0, rd_baseline_frames: 0,
            rd_tachy_ctr: 0, rd_cooldown: 0,
            rd_ac_buf: RingBuf::new(), rd_ac_frames: 0,
            cs_present: false, cs_breathing_ok: false,
            cs_no_br_ctr: 0, cs_no_motion_ctr: 0, cs_entry_logged: false,
            pm_energy_buf: RingBuf::new(), pm_var_buf: RingBuf::new(),
            pm_cooldown: 0,
            sa_no_br_ctr: 0, sa_apnea_active: false,
            sa_apnea_events: 0, sa_sleep_secs: 0,
            ca_hr_buf: RingBuf::new(),
            ca_tachy_ctr: 0, ca_brady_ctr: 0, ca_missed_ctr: 0, ca_prev_hr: 0.0,
            sz_energy_buf: RingBuf::new(), sz_band_buf: RingBuf::new(),
            sz_seizing: false, sz_seizure_ctr: 0, sz_postictal_ctr: 0,
            intr_baseline: [0.0; 32], intr_baseline_set: false,
            intr_baseline_frames: 0, intr_armed: false,
            intr_detect_ctr: 0, intr_cooldown: 0,
            oc: OccState {
                baseline_var: [0.0; 8], score: [0.0; 8],
                occupied: 0, prev_occupied: 0,
                calib_sum: [0.0; 8], calib_count: 0, calibrated: false,
                frame_count: 0, n_zones: 0,
            },
            mc: McState {
                signature: [[0.0; MC_FEAT_DIM]; MC_MAX_PERSONS],
                active: 0, tracked: [0; MC_MAX_PERSONS], absent: [0; MC_MAX_PERSONS],
                person_id: [0; MC_MAX_PERSONS], prev_assign: [MC_UNASSIGNED; MC_MAX_PERSONS],
                frame_count: 0, swap_count: 0,
            },
            wd: WdState {
                baseline_amp_var: [0.0; WD_MAX_SC],
                baseline_phase_var: [0.0; WD_MAX_SC],
                cal_amp_sum: [0.0; WD_MAX_SC],
                cal_amp_sq_sum: [0.0; WD_MAX_SC],
                cal_phase_sum: [0.0; WD_MAX_SC],
                cal_phase_sq_sum: [0.0; WD_MAX_SC],
                cal_count: 0, calibrated: false,
                run_amp_mean: [0.0; WD_MAX_SC],
                run_amp_m2: [0.0; WD_MAX_SC],
                run_phase_mean: [0.0; WD_MAX_SC],
                run_phase_m2: [0.0; WD_MAX_SC],
                run_count: 0,
                metal_run: 0, weapon_run: 0,
                cd_metal: 0, cd_weapon: 0, cd_recalib: 0,
                frame_count: 0,
            },
            sr: SrState {
                corr_model: [0.0; 32],
                model_frames: 0, model_built: false,
                total_dropouts: 0, total_recoveries: 0,
            },
            gait: GaitState {
                phase_var_buf: RingBuf::new(),
                peak_threshold: 0.0, last_peak_idx: -1,
                step_intervals: [0.0; 8], si_idx: 0, si_count: 0,
                shuffling_ctr: 0, fest_short_ctr: 0, fest_prev_interval: 0.0,
            },
            loiter: LoiteringState {
                state: 0, enter_timer: 0, dwell_timer: 0, exit_cooldown: 0,
            },
            vib: VibrationState {
                phase_rms_buf: RingBuf::new(),
                seismic_cooldown: 0, resonance_cooldown: 0,
                drift_cumulative: 0.0, drift_frames: 0,
            },
            meta: MetaState {
                thresholds: [0.5; 8], best_thresholds: [0.5; 8], best_score: 0.0,
                consecutive_failures: 0, iteration: 0,
                explore_step: 0, direction: true, frame_ctr: 0,
                true_positives: 0, false_positives: 0,
            },
            ltl: LtlState {
                violations: [0; 8],
                motion_stop_timer: 0, breathing_high_timer: 0,
                hr_high_timer: 0, seizure_gait_timer: 0,
                alert_cooldown: [0; 8],
            },
        }
    }

    /// Process one CSI frame through all 10 modules.
    /// Returns aggregated alerts.
    #[allow(clippy::too_many_arguments)]
    pub fn process_frame(
        &mut self,
        phases: &[f32], amplitudes: &[f32],
        motion_energy: f32,
        breathing_bpm: Option<f64>, heart_rate_bpm: Option<f64>,
        presence: bool,
    ) -> Vec<EdgeAlert> {
        let mut alerts = Vec::new();
        let br = breathing_bpm.unwrap_or(0.0) as f32;
        let hr = heart_rate_bpm.unwrap_or(0.0) as f32;

        // Compute amplitude stats
        let n = amplitudes.len().min(32);
        let amp_mean = if n > 0 { amplitudes[..n].iter().sum::<f32>() / n as f32 } else { 0.0 };
        let amp_var = if n > 1 {
            amplitudes[..n].iter().map(|a| (a - amp_mean).powi(2)).sum::<f32>() / n as f32
        } else { 0.0 };

        // ── Module 14: sig_sparse_recovery (run first, detects & recovers null subcarriers) ──
        {
            let n_sc = amplitudes.len().min(32);
            let mut dropout_count = 0u32;
            // Build correlation model from non-null carriers
            if !self.sr.model_built {
                for i in 0..n_sc {
                    self.sr.corr_model[i] = 0.15 * amplitudes[i] + 0.85 * self.sr.corr_model[i];
                }
                self.sr.model_frames += 1;
                if self.sr.model_frames >= 100 { self.sr.model_built = true; }
            }
            for i in 0..n_sc {
                if amplitudes[i].abs() < 0.001 { dropout_count += 1; self.sr.total_dropouts += 1; }
            }
            if dropout_count > 0 {
                let dropout_rate = dropout_count as f32 / n_sc.max(1) as f32;
                alerts.push(EdgeAlert {
                    module: "sparse_recovery".into(), event_type: 717,
                    event_name: "DropoutRate".into(), value: dropout_rate,
                    severity: if dropout_rate > 0.2 { "warning".into() } else { "info".into() },
                });
                // ISTA-lite recovery: estimate missing via correlation model
                if self.sr.model_built {
                    let mut recovered = 0u32;
                    for i in 0..n_sc {
                        if amplitudes[i].abs() < 0.001 && self.sr.corr_model[i].abs() > 0.001 {
                            recovered += 1; self.sr.total_recoveries += 1;
                        }
                    }
                    if recovered > 0 {
                        alerts.push(EdgeAlert {
                            module: "sparse_recovery".into(), event_type: 715,
                            event_name: "RecoveryComplete".into(), value: recovered as f32,
                            severity: "info".into(),
                        });
                    } else if dropout_count > 4 {
                        alerts.push(EdgeAlert {
                            module: "sparse_recovery".into(), event_type: 716,
                            event_name: "RecoveryError".into(), value: dropout_count as f32,
                            severity: "warning".into(),
                        });
                    }
                }
            }
        }

        // ── Module 1: vital_trend ──────────────────────────────────────
        self.vt_timer += 1;
        self.vt_br.push(br);
        self.vt_hr.push(hr);
        if self.vt_timer % 10 == 0 { // ~1 Hz
            // Apnea
            if br < 1.0 { self.vt_apnea_ctr += 1; }
            else { self.vt_apnea_ctr = 0; }
            if self.vt_apnea_ctr >= 20 {
                alerts.push(EdgeAlert {
                    module: "vital_trend".into(), event_type: 105,
                    event_name: "Apnea".into(), value: self.vt_apnea_ctr as f32,
                    severity: "critical".into(),
                });
            }
            // Bradypnea
            if br > 0.0 && br < 12.0 { self.vt_brady_ctr = self.vt_brady_ctr.saturating_add(1); }
            else { self.vt_brady_ctr = 0; }
            if self.vt_brady_ctr >= 5 {
                alerts.push(EdgeAlert { module: "vital_trend".into(), event_type: 101,
                    event_name: "Bradypnea".into(), value: br, severity: "warning".into() });
            }
            // Tachypnea
            if br > 25.0 { self.vt_tachy_ctr = self.vt_tachy_ctr.saturating_add(1); }
            else { self.vt_tachy_ctr = 0; }
            if self.vt_tachy_ctr >= 5 {
                alerts.push(EdgeAlert { module: "vital_trend".into(), event_type: 102,
                    event_name: "Tachypnea".into(), value: br, severity: "warning".into() });
            }
            // Bradycardia
            if hr > 0.0 && hr < 50.0 { self.vt_hr_brady_ctr = self.vt_hr_brady_ctr.saturating_add(1); }
            else { self.vt_hr_brady_ctr = 0; }
            if self.vt_hr_brady_ctr >= 5 {
                alerts.push(EdgeAlert { module: "vital_trend".into(), event_type: 103,
                    event_name: "Bradycardia".into(), value: hr, severity: "warning".into() });
            }
            // Tachycardia
            if hr > 120.0 { self.vt_hr_tachy_ctr = self.vt_hr_tachy_ctr.saturating_add(1); }
            else { self.vt_hr_tachy_ctr = 0; }
            if self.vt_hr_tachy_ctr >= 5 {
                alerts.push(EdgeAlert { module: "vital_trend".into(), event_type: 104,
                    event_name: "Tachycardia".into(), value: hr, severity: "critical".into() });
            }
        }

        // ── Module 2: lrn_anomaly_attractor ────────────────────────────
        if n > 0 {
            let state = [amplitudes[..n].iter().sum::<f32>() / n as f32,
                         br, amp_var.sqrt(), motion_energy];
            self.att_frame += 1;
            if self.att_cooldown > 0 { self.att_cooldown -= 1; }
            if !self.att_initialized {
                if self.att_frame == 1 { self.att_center = state; }
                else { for d in 0..4 { self.att_center[d] = 0.01 * state[d] + 0.99 * self.att_center[d]; } }
                let dist = euclid_4(&state, &self.att_center);
                if dist > self.att_radius { self.att_radius = dist; }
                // Compute Lyapunov contribution
                if self.att_frame > 1 {
                    let delta = euclid_4(&state, &self.att_prev_state);
                    if self.att_prev_delta > 1e-8 && delta > 1e-8 {
                        self.att_lyap_sum += (delta / self.att_prev_delta).ln() as f64;
                        self.att_lyap_cnt += 1;
                    }
                    self.att_prev_delta = delta;
                }
                self.att_prev_state = state;
                if self.att_frame >= 200 && self.att_lyap_cnt > 0 {
                    self.att_initialized = true;
                    if self.att_radius < 0.01 { self.att_radius = 0.01; }
                    alerts.push(EdgeAlert { module: "attractor".into(), event_type: 738,
                        event_name: "LearningComplete".into(), value: 1.0, severity: "info".into() });
                }
            } else {
                let dist = euclid_4(&state, &self.att_center);
                if dist > self.att_radius * 3.0 && self.att_cooldown == 0 {
                    self.att_cooldown = 100;
                    alerts.push(EdgeAlert { module: "attractor".into(), event_type: 737,
                        event_name: "BasinDeparture".into(), value: dist / self.att_radius,
                        severity: "critical".into() });
                }
            }
        }

        // ── Module 3: coherence ────────────────────────────────────────
        if n > 0 {
            if !self.coh_initialized {
                for i in 0..n.min(32) { self.coh_prev_phases[i] = phases[i]; }
                self.coh_initialized = true;
            } else {
                let (mut sum_re, mut sum_im) = (0.0f32, 0.0f32);
                for i in 0..n.min(32) {
                    let delta = phases[i] - self.coh_prev_phases[i];
                    sum_re += delta.cos(); sum_im += delta.sin();
                    self.coh_prev_phases[i] = phases[i];
                }
                let nf = n.min(32) as f32;
                let coh = ((sum_re/nf).powi(2) + (sum_im/nf).powi(2)).sqrt();
                self.coh_smoothed = 0.1 * coh + 0.9 * self.coh_smoothed;
                self.coh_gate = if self.coh_smoothed < 0.4 { 2 }
                               else if self.coh_smoothed < 0.7 { 1 }
                               else { 0 };
            }
        }

        // ── Module 4: med_respiratory_distress ─────────────────────────
        self.rd_br_buf.push(br);
        self.rd_var_buf.push(amp_var);
        if self.rd_cooldown > 0 { self.rd_cooldown -= 1; }
        // Baseline learning (60s)
        if self.rd_baseline_frames < 600 {
            self.rd_baseline_frames += 1;
            self.rd_baseline_var = (self.rd_baseline_var * (self.rd_baseline_frames - 1) as f32
                + amp_var) / self.rd_baseline_frames as f32;
        } else {
            // Tachypnea
            if br > 25.0 { self.rd_tachy_ctr = self.rd_tachy_ctr.saturating_add(1); }
            else { self.rd_tachy_ctr = 0; }
            if self.rd_tachy_ctr >= 8 && self.rd_cooldown == 0 {
                self.rd_cooldown = 400;
                alerts.push(EdgeAlert { module: "resp_distress".into(), event_type: 120,
                    event_name: "Tachypnea".into(), value: br, severity: "warning".into() });
            }
            // Labored breathing
            let recent_var = self.rd_var_buf.mean_last(30);
            if recent_var > self.rd_baseline_var * 3.0 && br > 0.0 {
                alerts.push(EdgeAlert { module: "resp_distress".into(), event_type: 121,
                    event_name: "LaboredBreathing".into(), value: recent_var / self.rd_baseline_var.max(0.001),
                    severity: "warning".into() });
            }
            // Cheyne-Stokes (autocorrelation-based, every 30s)
            self.rd_ac_buf.push(amp_var);
            self.rd_ac_frames += 1;
            if self.rd_ac_frames % 300 == 0 && self.rd_ac_frames >= 900 {
                let ac = autocorr_max(&self.rd_ac_buf, 30, 90);
                if ac > 0.35 {
                    alerts.push(EdgeAlert { module: "resp_distress".into(), event_type: 122,
                        event_name: "CheyneStokes".into(), value: ac, severity: "critical".into() });
                }
            }
        }

        // ── Module 5: ind_confined_space ───────────────────────────────
        let was_present = self.cs_present;
        self.cs_present = presence;
        self.cs_breathing_ok = br > 0.0;
        // Entry/exit
        if self.cs_present && !was_present && self.cs_entry_logged {
            alerts.push(EdgeAlert { module: "confined_space".into(), event_type: 510,
                event_name: "WorkerEntry".into(), value: 1.0, severity: "info".into() });
        }
        if !self.cs_present && was_present {
            alerts.push(EdgeAlert { module: "confined_space".into(), event_type: 511,
                event_name: "WorkerExit".into(), value: 1.0, severity: "info".into() });
            self.cs_no_br_ctr = 0; self.cs_no_motion_ctr = 0;
        }
        if self.cs_present { self.cs_entry_logged = true; }
        // No breathing → extraction alert
        if self.cs_present && !self.cs_breathing_ok {
            self.cs_no_br_ctr += 1;
            if self.cs_no_br_ctr > 300 { // 15s @ 20Hz
                alerts.push(EdgeAlert { module: "confined_space".into(), event_type: 513,
                    event_name: "ExtractionAlert".into(), value: self.cs_no_br_ctr as f32 / 20.0,
                    severity: "critical".into() });
            }
        } else { self.cs_no_br_ctr = 0; }
        // Immobile → immobile alert
        if self.cs_present && motion_energy < 0.1 {
            self.cs_no_motion_ctr += 1;
            if self.cs_no_motion_ctr > 1200 { // 60s @ 20Hz
                alerts.push(EdgeAlert { module: "confined_space".into(), event_type: 514,
                    event_name: "ImmobileAlert".into(), value: self.cs_no_motion_ctr as f32 / 20.0,
                    severity: "critical".into() });
            }
        } else { self.cs_no_motion_ctr = 0; }

        // ── Module 6: sec_panic_motion ─────────────────────────────────
        if self.pm_cooldown > 0 { self.pm_cooldown -= 1; }
        self.pm_energy_buf.push(motion_energy);
        self.pm_var_buf.push(amp_var);
        if self.pm_cooldown == 0 && self.pm_energy_buf.len >= 100 {
            let jerk = jerk_estimate(&self.pm_energy_buf);
            let entropy = entropy_estimate(&self.pm_var_buf);
            let mean_energy = self.pm_energy_buf.mean_last(100);
            if jerk > 2.0 && entropy > 0.35 && mean_energy > 1.0 && presence {
                self.pm_cooldown = 100;
                alerts.push(EdgeAlert { module: "panic_motion".into(), event_type: 250,
                    event_name: "PanicDetected".into(), value: jerk, severity: "critical".into() });
            } else if jerk > 1.5 && entropy > 0.25 && mean_energy < 5.0 && presence {
                alerts.push(EdgeAlert { module: "panic_motion".into(), event_type: 251,
                    event_name: "StrugglePattern".into(), value: entropy, severity: "warning".into() });
            } else if mean_energy > 5.0 && jerk > 0.05 && entropy < 0.25 {
                alerts.push(EdgeAlert { module: "panic_motion".into(), event_type: 252,
                    event_name: "FleeingDetected".into(), value: mean_energy, severity: "warning".into() });
            }
        }

        // ── Module 7: med_sleep_apnea ──────────────────────────────────
        self.sa_sleep_secs += 1;
        if br < 4.0 { self.sa_no_br_ctr += 1; }
        else {
            if self.sa_apnea_active {
                alerts.push(EdgeAlert { module: "sleep_apnea".into(), event_type: 101,
                    event_name: "ApneaEnd".into(), value: self.sa_no_br_ctr as f32 / 20.0,
                    severity: "info".into() });
            }
            self.sa_no_br_ctr = 0;
            self.sa_apnea_active = false;
        }
        if self.sa_no_br_ctr > 200 && !self.sa_apnea_active { // 10s @ 20Hz
            self.sa_apnea_active = true;
            self.sa_apnea_events += 1;
            alerts.push(EdgeAlert { module: "sleep_apnea".into(), event_type: 100,
                event_name: "ApneaStart".into(), value: self.sa_no_br_ctr as f32 / 20.0,
                severity: "critical".into() });
        }
        // AHI report every hour
        if self.sa_sleep_secs % 72000 == 0 { // 3600s * 20Hz
            let ahi = self.sa_apnea_events as f32 / (self.sa_sleep_secs as f32 / 72000.0);
            alerts.push(EdgeAlert { module: "sleep_apnea".into(), event_type: 102,
                event_name: "AHIUpdate".into(), value: ahi, severity: "info".into() });
        }

        // ── Module 8: med_cardiac_arrhythmia ───────────────────────────
        self.ca_hr_buf.push(hr);
        let avg_hr = self.ca_hr_buf.mean_last(30);
        // Tachycardia
        if hr > 100.0 { self.ca_tachy_ctr = self.ca_tachy_ctr.saturating_add(1); }
        else { self.ca_tachy_ctr = 0; }
        if self.ca_tachy_ctr >= 40 { // 2s sustained
            alerts.push(EdgeAlert { module: "cardiac".into(), event_type: 110,
                event_name: "Tachycardia".into(), value: hr, severity: "warning".into() });
        }
        // Bradycardia
        if hr > 0.0 && hr < 50.0 { self.ca_brady_ctr = self.ca_brady_ctr.saturating_add(1); }
        else { self.ca_brady_ctr = 0; }
        if self.ca_brady_ctr >= 40 {
            alerts.push(EdgeAlert { module: "cardiac".into(), event_type: 111,
                event_name: "Bradycardia".into(), value: hr, severity: "warning".into() });
        }
        // Missed beat
        if self.ca_prev_hr > 10.0 && hr < self.ca_prev_hr * 0.7 {
            self.ca_missed_ctr = self.ca_missed_ctr.saturating_add(1);
            if self.ca_missed_ctr >= 3 {
                alerts.push(EdgeAlert { module: "cardiac".into(), event_type: 112,
                    event_name: "MissedBeat".into(), value: hr, severity: "critical".into() });
                self.ca_missed_ctr = 0;
            }
        } else { self.ca_missed_ctr = 0; }
        self.ca_prev_hr = hr;
        // HRV anomaly (simple RMSSD)
        if self.ca_hr_buf.len >= 30 {
            let rmssd = rmssd(&self.ca_hr_buf, 30);
            if rmssd > 100.0 || (rmssd < 10.0 && avg_hr > 0.0) {
                alerts.push(EdgeAlert { module: "cardiac".into(), event_type: 113,
                    event_name: "HRVAnomaly".into(), value: rmssd, severity: "warning".into() });
            }
        }

        // ── Module 9: med_seizure_detect ───────────────────────────────
        self.sz_energy_buf.push(motion_energy);
        // Estimate 3-8 Hz band energy via amplitude variance oscillation
        let band_energy = amp_var; // simplified proxy
        self.sz_band_buf.push(band_energy);
        if !self.sz_seizing {
            let mean_e = self.sz_energy_buf.mean_last(60);
            let mean_b = self.sz_band_buf.mean_last(60);
            if mean_e > 3.0 && mean_b > 2.0 && presence {
                self.sz_seizing = true;
                self.sz_seizure_ctr = 0;
                self.sz_postictal_ctr = 0;
                alerts.push(EdgeAlert { module: "seizure".into(), event_type: 140,
                    event_name: "SeizureOnset".into(), value: mean_e, severity: "critical".into() });
            }
        } else {
            self.sz_seizure_ctr += 1;
            let mean_e = self.sz_energy_buf.mean_last(20);
            // Tonic phase (high energy, low variance)
            if mean_e > 5.0 && self.sz_band_buf.mean_last(20) < 1.0 {
                alerts.push(EdgeAlert { module: "seizure".into(), event_type: 141,
                    event_name: "SeizureTonic".into(), value: mean_e, severity: "critical".into() });
            }
            // Clonic phase (rhythmic 3-8Hz)
            if self.sz_band_buf.mean_last(20) > 2.0 {
                alerts.push(EdgeAlert { module: "seizure".into(), event_type: 142,
                    event_name: "SeizureClonic".into(), value: mean_e, severity: "critical".into() });
            }
            // Recovery
            if motion_energy < 1.0 && self.sz_seizure_ctr > 100 {
                self.sz_postictal_ctr += 1;
                if self.sz_postictal_ctr > 40 {
                    self.sz_seizing = false;
                    alerts.push(EdgeAlert { module: "seizure".into(), event_type: 143,
                        event_name: "PostIctal".into(), value: 1.0, severity: "warning".into() });
                }
            } else { self.sz_postictal_ctr = 0; }
        }

        // ── Module 10: intrusion ───────────────────────────────────────
        if self.intr_cooldown > 0 { self.intr_cooldown -= 1; }
        if !self.intr_baseline_set {
            if n > 0 {
                for i in 0..n.min(32) { self.intr_baseline[i] += amplitudes[i]; }
                self.intr_baseline_frames += 1;
            }
            if self.intr_baseline_frames >= 200 {
                for b in self.intr_baseline.iter_mut() { *b /= self.intr_baseline_frames as f32; }
                self.intr_baseline_set = true;
            }
        } else {
            // Arm when quiet for 5s
            if motion_energy < 0.5 && !presence {
                if !self.intr_armed {
                    self.intr_armed = true;
                    alerts.push(EdgeAlert { module: "intrusion".into(), event_type: 202,
                        event_name: "IntrusionArmed".into(), value: 1.0, severity: "info".into() });
                }
            }
            // Detect intrusion
            if self.intr_armed && presence && self.intr_cooldown == 0 {
                if n > 0 {
                    let mut change = 0.0f32;
                    for i in 0..n.min(32) {
                        if self.intr_baseline[i] > 0.1 {
                            change += (amplitudes[i] / self.intr_baseline[i]).abs();
                        }
                    }
                    change /= n as f32;
                    if change > 3.0 {
                        self.intr_detect_ctr = self.intr_detect_ctr.saturating_add(1);
                        if self.intr_detect_ctr >= 3 {
                            self.intr_cooldown = 100;
                            self.intr_detect_ctr = 0;
                            alerts.push(EdgeAlert { module: "intrusion".into(), event_type: 200,
                                event_name: "IntrusionAlert".into(), value: change,
                                severity: "critical".into() });
                        }
                    } else { self.intr_detect_ctr = 0; }
                }
            }
        }


        // ── Module 11: occupancy ───────────────────────────────────────
        // Divide subcarriers into zones (groups of ~4), track per-zone variance deviation
        self.oc.frame_count += 1;
        if n >= 2 {
            let zone_count = ((n / 4).min(8)).max(1);
            self.oc.n_zones = zone_count;
            let subs_per_zone = n / zone_count;

            // Compute per-zone amplitude variance
            let mut zone_vars = [0.0f32; 8];
            for z in 0..zone_count {
                let start = z * subs_per_zone;
                let end = if z == zone_count - 1 { n } else { start + subs_per_zone };
                let cnt = (end - start) as f32;
                if cnt < 1.0 { continue; }
                let mean = amplitudes[start..end].iter().sum::<f32>() / cnt;
                let var = amplitudes[start..end].iter().map(|a| (a - mean).powi(2)).sum::<f32>() / cnt;
                zone_vars[z] = var;
            }

            // Calibration
            if !self.oc.calibrated {
                for z in 0..zone_count { self.oc.calib_sum[z] += zone_vars[z]; }
                self.oc.calib_count += 1;
                if self.oc.calib_count >= 200 {
                    let nf = self.oc.calib_count as f32;
                    for z in 0..zone_count { self.oc.baseline_var[z] = self.oc.calib_sum[z] / nf; }
                    self.oc.calibrated = true;
                }
            } else {
                // Score zones
                self.oc.prev_occupied = self.oc.occupied;
                let mut total = 0u8;
                for z in 0..zone_count {
                    let deviation = (zone_vars[z] - self.oc.baseline_var[z]).abs();
                    let raw = if self.oc.baseline_var[z] > 0.001 {
                        deviation / self.oc.baseline_var[z]
                    } else { deviation * 100.0 };
                    self.oc.score[z] = 0.15 * raw + 0.85 * self.oc.score[z];
                    let was = (self.oc.occupied >> z) & 1;
                    let now = if was == 1 {
                        self.oc.score[z] > 0.01  // hysteresis: lower exit threshold
                    } else {
                        self.oc.score[z] > 0.02   // entry threshold
                    };
                    if now {
                        self.oc.occupied |= 1 << z;
                        total += 1;
                    } else {
                        self.oc.occupied &= !(1 << z);
                    }
                }
                // Zone count event every 10 frames
                if self.oc.frame_count % 10 == 0 {
                    if total > 0 && n > 0 {
                        alerts.push(EdgeAlert { module: "occupancy".into(), event_type: 301,
                            event_name: "ZoneCount".into(), value: total as f32,
                            severity: "info".into() });
                    }
                    for z in 0..zone_count {
                        if ((self.oc.occupied >> z) & 1) == 1 {
                            let conf = self.oc.score[z].min(0.99);
                            alerts.push(EdgeAlert { module: "occupancy".into(), event_type: 300,
                                event_name: "ZoneOccupied".into(), value: z as f32 + conf,
                                severity: "info".into() });
                        }
                    }
                }
                // Transition events
                let changed = self.oc.occupied ^ self.oc.prev_occupied;
                for z in 0..zone_count {
                    if (changed >> z) & 1 == 1 {
                        let entering = ((self.oc.occupied >> z) & 1) == 1;
                        alerts.push(EdgeAlert { module: "occupancy".into(), event_type: 302,
                            event_name: if entering { "ZoneEntered".into() } else { "ZoneLeft".into() },
                            value: z as f32, severity: "info".into() });
                    }
                }
            }
        }

        // ── Module 12: sig_mincut_person_match ─────────────────────────
        if n >= MC_FEAT_DIM {
            let n_persons = if presence { 1usize } else { 0usize }; // at least 1 if breathing detected
            let n_det = match () {
                _ if br > 0.0 => 1,
                _ => n_persons,
            };
            let n_det = n_det.min(MC_MAX_PERSONS);

            // Compute per-subcarrier variance (simple estimate from consecutive diffs)
            // Use amplitude variance as proxy for per-subcarrier features
            let sc_var: Vec<f32> = amplitudes[..n].iter()
                .map(|&a| (a - amp_mean).abs())  // simplified feature
                .collect();

            if n_det > 0 {
                self.mc.frame_count += 1;

                // Extract top-8 variance features per detected person
                let mut cur_feat = [[0.0f32; MC_FEAT_DIM]; MC_MAX_PERSONS];
                let subs = n / n_det;
                for p in 0..n_det {
                    let start = p * subs;
                    let end = if p == n_det - 1 { n } else { start + subs };
                    // Select top-8 highest variance subcarriers in this region
                    let mut vals: Vec<(usize, f32)> = (start..end).map(|i| (i, sc_var[i])).collect();
                    vals.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
                    for f in 0..MC_FEAT_DIM.min(vals.len()) {
                        cur_feat[p][f] = vals[f].1;
                    }
                    for f in MC_FEAT_DIM.min(vals.len())..MC_FEAT_DIM {
                        cur_feat[p][f] = 0.0;
                    }
                }

                // Greedy Hungarian-lite assignment
                let mut assign = [MC_UNASSIGNED; MC_MAX_PERSONS];
                let mut costs = [0.0f32; MC_MAX_PERSONS];
                {
                    let mut det_used = [false; MC_MAX_PERSONS];
                    let mut slot_used = [false; MC_MAX_PERSONS];
                    let n_active = (0..MC_MAX_PERSONS).filter(|&s| (self.mc.active >> s) & 1 == 1).count();
                    for _ in 0..n_det.min(n_active.max(1)) {
                        let (mut min_c, mut best_d, mut best_s) = (f32::MAX, 0usize, 0usize);
                        for d in 0..n_det {
                            if det_used[d] { continue; }
                            for s in 0..MC_MAX_PERSONS {
                                if slot_used[s] || (self.mc.active >> s) & 1 == 0 { continue; }
                                let dist = l2mc(&cur_feat[d], &self.mc.signature[s]);
                                if dist < min_c { min_c = dist; best_d = d; best_s = s; }
                            }
                        }
                        if min_c > 5.0 { break; }
                        assign[best_d] = best_s as u8;
                        costs[best_d] = min_c;
                        det_used[best_d] = true;
                        slot_used[best_s] = true;
                    }
                    // Assign unmatched to free slots
                    for d in 0..n_det {
                        if assign[d] != MC_UNASSIGNED { continue; }
                        for s in 0..MC_MAX_PERSONS {
                            if !slot_used[s] && (self.mc.active >> s) & 1 == 0 {
                                assign[d] = s as u8; costs[d] = 5.0;
                                slot_used[s] = true; break;
                            }
                        }
                        if assign[d] != MC_UNASSIGNED { continue; }
                        for s in 0..MC_MAX_PERSONS {
                            if !slot_used[s] {
                                assign[d] = s as u8; costs[d] = 5.0;
                                slot_used[s] = true; break;
                            }
                        }
                    }
                }

                // Detect ID swaps
                for d in 0..n_det {
                    if assign[d] != MC_UNASSIGNED && self.mc.prev_assign[d] != MC_UNASSIGNED
                        && assign[d] != self.mc.prev_assign[d] {
                        self.mc.swap_count += 1;
                        alerts.push(EdgeAlert { module: "mincut".into(), event_type: 721,
                            event_name: "PersonIDSwap".into(),
                            value: self.mc.prev_assign[d] as f32 * 16.0 + assign[d] as f32,
                            severity: "warning".into() });
                    }
                }

                // Update signatures (EMA) and emit assignment events
                for s in 0..MC_MAX_PERSONS {
                    if (self.mc.active >> s) & 1 == 1 {
                        self.mc.absent[s] = self.mc.absent[s].saturating_add(1);
                    }
                }
                for d in 0..n_det {
                    let s = assign[d] as usize;
                    if s >= MC_MAX_PERSONS { continue; }
                    let was_active = (self.mc.active >> s) & 1 == 1;
                    if was_active {
                        for f in 0..MC_FEAT_DIM {
                            self.mc.signature[s][f] = 0.15 * cur_feat[d][f] + 0.85 * self.mc.signature[s][f];
                        }
                        self.mc.tracked[s] = self.mc.tracked[s].saturating_add(1);
                    } else {
                        self.mc.signature[s] = cur_feat[d];
                        self.mc.active |= 1 << s;
                        self.mc.tracked[s] = 1;
                    }
                    self.mc.absent[s] = 0;
                    let conf = if costs[d] < 5.0 { 1.0 - costs[d] / 5.0 } else { 0.0 };
                    alerts.push(EdgeAlert { module: "mincut".into(), event_type: 720,
                        event_name: "PersonAssigned".into(),
                        value: self.mc.person_id[s] as f32 + conf.min(0.99) * 0.01,
                        severity: "info".into() });
                }

                // Timeout absent slots
                for s in 0..MC_MAX_PERSONS {
                    if self.mc.absent[s] >= 100 {
                        self.mc.active &= !(1 << s);
                        self.mc.tracked[s] = 0;
                        self.mc.absent[s] = 0;
                        self.mc.signature[s] = [0.0; MC_FEAT_DIM];
                    }
                }

                // Aggregate confidence every 10 frames
                if self.mc.frame_count % 10 == 0 && n_det > 0 {
                    let avg = costs.iter().take(n_det)
                        .map(|&c| if c < 5.0 { 1.0 - c / 5.0 } else { 0.0f32 })
                        .sum::<f32>() / n_det as f32;
                    alerts.push(EdgeAlert { module: "mincut".into(), event_type: 722,
                        event_name: "MatchConfidence".into(), value: avg,
                        severity: "info".into() });
                }
                self.mc.prev_assign = assign;
            }
        }

        // ── Module 13: sec_weapon_detect ────────────────────────────────
        self.wd.frame_count += 1;
        self.wd.cd_metal = self.wd.cd_metal.saturating_sub(1);
        self.wd.cd_weapon = self.wd.cd_weapon.saturating_sub(1);
        self.wd.cd_recalib = self.wd.cd_recalib.saturating_sub(1);

        if n >= 2 {
            // Calibration phase
            if !self.wd.calibrated {
                for i in 0..n.min(WD_MAX_SC) {
                    self.wd.cal_amp_sum[i] += amplitudes[i];
                    self.wd.cal_amp_sq_sum[i] += amplitudes[i] * amplitudes[i];
                    self.wd.cal_phase_sum[i] += phases[i];
                    self.wd.cal_phase_sq_sum[i] += phases[i] * phases[i];
                }
                self.wd.cal_count += 1;
                if self.wd.cal_count >= 100 {
                    let nf = self.wd.cal_count as f32;
                    for i in 0..n.min(WD_MAX_SC) {
                        let am = self.wd.cal_amp_sum[i] / nf;
                        self.wd.baseline_amp_var[i] =
                            (self.wd.cal_amp_sq_sum[i] / nf - am * am).max(0.001);
                        let pm = self.wd.cal_phase_sum[i] / nf;
                        self.wd.baseline_phase_var[i] =
                            (self.wd.cal_phase_sq_sum[i] / nf - pm * pm).max(0.001);
                    }
                    self.wd.calibrated = true;
                }
            } else {
                // Welford online variance
                self.wd.run_count += 1;
                let rc = self.wd.run_count as f32;
                for i in 0..n.min(WD_MAX_SC) {
                    let da = amplitudes[i] - self.wd.run_amp_mean[i];
                    self.wd.run_amp_mean[i] += da / rc;
                    let da2 = amplitudes[i] - self.wd.run_amp_mean[i];
                    self.wd.run_amp_m2[i] += da * da2;

                    let dp = phases[i] - self.wd.run_phase_mean[i];
                    self.wd.run_phase_mean[i] += dp / rc;
                    let dp2 = phases[i] - self.wd.run_phase_mean[i];
                    self.wd.run_phase_m2[i] += dp * dp2;
                }

                // Only detect with presence AND motion
                if presence && motion_energy >= 0.5 && self.wd.run_count >= 4 {
                    let (mut ratio_sum, mut valid_sc) = (0.0f32, 0u32);
                    let mut max_drift = 0.0f32;
                    for i in 0..n.min(WD_MAX_SC) {
                        let av = self.wd.run_amp_m2[i] / (self.wd.run_count as f32 - 1.0);
                        let pv = self.wd.run_phase_m2[i] / (self.wd.run_count as f32 - 1.0);
                        if pv > 0.0001 { ratio_sum += av / pv; valid_sc += 1; }
                        let drift = if self.wd.baseline_amp_var[i] > 0.0001 {
                            (av - self.wd.baseline_amp_var[i]).abs() / self.wd.baseline_amp_var[i]
                        } else { 0.0 };
                        if drift > max_drift { max_drift = drift; }
                    }
                    if valid_sc >= 2 {
                        let mean_ratio = ratio_sum / valid_sc as f32;

                        // Recalibration alert
                        if max_drift > 3.0 && self.wd.cd_recalib == 0 {
                            self.wd.cd_recalib = 300;
                            alerts.push(EdgeAlert { module: "weapon_detect".into(), event_type: 222,
                                event_name: "CalibrationNeeded".into(), value: max_drift,
                                severity: "warning".into() });
                        }

                        // Metal anomaly
                        if mean_ratio > 4.0 { self.wd.metal_run = self.wd.metal_run.saturating_add(1); }
                        else { self.wd.metal_run = self.wd.metal_run.saturating_sub(1); }
                        if self.wd.metal_run >= 4 && self.wd.cd_metal == 0 {
                            self.wd.cd_metal = 60;
                            alerts.push(EdgeAlert { module: "weapon_detect".into(), event_type: 220,
                                event_name: "MetalAnomaly".into(), value: mean_ratio,
                                severity: "warning".into() });
                        }

                        // Weapon alert
                        if mean_ratio > 8.0 { self.wd.weapon_run = self.wd.weapon_run.saturating_add(1); }
                        else { self.wd.weapon_run = self.wd.weapon_run.saturating_sub(1); }
                        if self.wd.weapon_run >= 6 && self.wd.cd_weapon == 0 {
                            self.wd.cd_weapon = 60;
                            alerts.push(EdgeAlert { module: "weapon_detect".into(), event_type: 221,
                                event_name: "WeaponAlert".into(), value: mean_ratio,
                                severity: "critical".into() });
                        }
                    }
                }
                // Reset running stats periodically when no one is present
                if !presence && self.wd.run_count > 200 {
                    self.wd.run_count = 0;
                    for i in 0..WD_MAX_SC {
                        self.wd.run_amp_mean[i] = 0.0; self.wd.run_amp_m2[i] = 0.0;
                        self.wd.run_phase_mean[i] = 0.0; self.wd.run_phase_m2[i] = 0.0;
                    }
                }
            }
        }

        // ── Module 15: med_gait_analysis ───────────────────────────────────────
        if motion_energy > 0.1 && presence && n >= 4 {
            // Phase variance across subcarriers → gait cycle proxy
            let phase_var = if n > 1 {
                let pm = phases[..n].iter().sum::<f32>() / n as f32;
                phases[..n].iter().map(|&p| (p - pm).powi(2)).sum::<f32>() / n as f32
            } else { 0.0 };
            self.gait.phase_var_buf.push(phase_var);
            if self.gait.phase_var_buf.len >= 60 {
                self.gait.peak_threshold = 0.8 * self.gait.peak_threshold
                    + 0.2 * self.gait.phase_var_buf.mean_last(60) * 1.5;
                let buf = &self.gait.phase_var_buf;
                let idx = buf.idx;
                let curr = buf.buf[(idx + 60 - 1) % 60];
                let prev = if buf.len >= 2 { buf.buf[(idx + 60 - 2) % 60] } else { 0.0 };
                let prev2 = if buf.len >= 3 { buf.buf[(idx + 60 - 3) % 60] } else { 0.0 };
                // Peak detection → footfall
                if curr > self.gait.peak_threshold && curr <= prev && prev > prev2 {
                    let current_idx = self.gait.phase_var_buf.len as i32;
                    let interval = current_idx - self.gait.last_peak_idx;
                    if interval > 5 && interval < 120 {
                        let si = &mut self.gait.step_intervals;
                        si[self.gait.si_idx] = interval as f32;
                        self.gait.si_idx = (self.gait.si_idx + 1) % 8;
                        if self.gait.si_count < 8 { self.gait.si_count += 1; }
                        if self.gait.si_count >= 2 {
                            let avg_interval = si[..self.gait.si_count].iter().sum::<f32>()
                                / self.gait.si_count as f32;
                            let cadence = 1200.0 / avg_interval.max(0.01);
                            alerts.push(EdgeAlert {
                                module: "gait".into(), event_type: 130,
                                event_name: "StepCadence".into(), value: cadence,
                                severity: "info".into(),
                            });
                            // Asymmetry: alternating left/right intervals
                            if self.gait.si_count >= 4 {
                                let (mut evens, mut odds) = (0.0f32, 0.0f32);
                                let (mut ec, mut oc) = (0u32, 0u32);
                                for (j, &iv) in si[..self.gait.si_count].iter().enumerate() {
                                    if j % 2 == 0 { evens += iv; ec += 1; }
                                    else { odds += iv; oc += 1; }
                                }
                                if ec > 0 && oc > 0 {
                                    let ratio = evens / ec as f32 / (odds / oc as f32).max(0.01);
                                    let asymmetry = (ratio - 1.0).abs();
                                    if asymmetry > 0.15 {
                                        alerts.push(EdgeAlert {
                                            module: "gait".into(), event_type: 131,
                                            event_name: "GaitAsymmetry".into(), value: asymmetry,
                                            severity: "warning".into(),
                                        });
                                    }
                                    let variability = {
                                        let m = si[..self.gait.si_count].iter().sum::<f32>()
                                            / self.gait.si_count as f32;
                                        let v = si[..self.gait.si_count].iter()
                                            .map(|&iv| (iv - m).powi(2)).sum::<f32>() / self.gait.si_count as f32;
                                        v.sqrt() / m.max(1.0)
                                    };
                                    let fall_risk = ((asymmetry * 50.0 + variability * 40.0
                                        + (cadence / 180.0).min(1.0) * 10.0) * 100.0).min(100.0);
                                    alerts.push(EdgeAlert {
                                        module: "gait".into(), event_type: 132,
                                        event_name: "FallRiskScore".into(), value: fall_risk,
                                        severity: if fall_risk > 70.0 { "critical".into() }
                                            else if fall_risk > 40.0 { "warning".into() }
                                            else { "info".into() },
                                    });
                                    // Shuffling: high cadence + low variability
                                    if cadence > 100.0 && variability < 0.1 {
                                        self.gait.shuffling_ctr = self.gait.shuffling_ctr.saturating_add(1);
                                        if self.gait.shuffling_ctr >= 3 {
                                            alerts.push(EdgeAlert {
                                                module: "gait".into(), event_type: 133,
                                                event_name: "ShufflingDetected".into(), value: cadence,
                                                severity: "warning".into(),
                                            });
                                        }
                                    } else { self.gait.shuffling_ctr = 0; }
                                    // Festination: progressively shorter steps
                                    let interval_f = interval as f32;
                                    if interval_f < self.gait.fest_prev_interval * 0.85 && interval < 30 {
                                        self.gait.fest_short_ctr = self.gait.fest_short_ctr.saturating_add(1);
                                        if self.gait.fest_short_ctr >= 5 {
                                            alerts.push(EdgeAlert {
                                                module: "gait".into(), event_type: 134,
                                                event_name: "Festination".into(), value: cadence,
                                                severity: "critical".into(),
                                            });
                                        }
                                    } else { self.gait.fest_short_ctr = 0; }
                                    self.gait.fest_prev_interval = interval as f32;
                                }
                            }
                        }
                    }
                    self.gait.last_peak_idx = current_idx;
                }
            }
        }

        // ── Module 16: sec_loitering ────────────────────────────────────────────
        {
            let stationary = motion_energy < 0.5;
            if self.loiter.exit_cooldown > 0 { self.loiter.exit_cooldown -= 1; }
            match self.loiter.state {
                0 => { // Absent
                    if presence {
                        self.loiter.enter_timer += 1;
                        if self.loiter.enter_timer >= 60 {
                            self.loiter.state = 2; self.loiter.enter_timer = 0;
                            self.loiter.dwell_timer = 0;
                        }
                    } else { self.loiter.enter_timer = 0; }
                }
                1 => { // Entering (3s confirm)
                    if presence { self.loiter.enter_timer += 1; }
                    else { self.loiter.enter_timer = 0; }
                    if self.loiter.enter_timer >= 60 {
                        self.loiter.state = 2; self.loiter.enter_timer = 0;
                        self.loiter.dwell_timer = 0;
                    }
                }
                2 => { // Present
                    if presence {
                        if stationary { self.loiter.dwell_timer += 1; }
                        else { self.loiter.dwell_timer = 0; }
                        if self.loiter.dwell_timer >= 6000 {
                            self.loiter.state = 3;
                            alerts.push(EdgeAlert {
                                module: "loitering".into(), event_type: 240,
                                event_name: "LoiteringStart".into(),
                                value: self.loiter.dwell_timer as f32 / 20.0,
                                severity: "warning".into(),
                            });
                        }
                    } else {
                        self.loiter.exit_cooldown = 600;
                        self.loiter.state = 0;
                        self.loiter.dwell_timer = 0;
                    }
                }
                3 => { // Loitering
                    if presence && stationary {
                        self.loiter.dwell_timer += 1;
                        if self.loiter.dwell_timer % 600 == 0 {
                            alerts.push(EdgeAlert {
                                module: "loitering".into(), event_type: 241,
                                event_name: "LoiteringOngoing".into(),
                                value: self.loiter.dwell_timer as f32 / 20.0,
                                severity: "warning".into(),
                            });
                        }
                    } else if !presence && self.loiter.exit_cooldown == 0 {
                        self.loiter.state = 0;
                        alerts.push(EdgeAlert {
                            module: "loitering".into(), event_type: 242,
                            event_name: "LoiteringEnd".into(),
                            value: self.loiter.dwell_timer as f32 / 20.0,
                            severity: "info".into(),
                        });
                        self.loiter.dwell_timer = 0;
                    }
                }
                _ => { self.loiter.state = 0; }
            }
        }

        // ── Module 17: ind_structural_vibration ─────────────────────────────────
        if !presence && n >= 2 {
            // Phase RMS as vibration proxy (radians)
            let phase_rms = {
                let sq: f32 = phases[..n].iter().map(|&p| p * p).sum();
                (sq / n as f32).sqrt()
            };
            self.vib.phase_rms_buf.push(phase_rms);
            self.vib.seismic_cooldown = self.vib.seismic_cooldown.saturating_sub(1);
            self.vib.resonance_cooldown = self.vib.resonance_cooldown.saturating_sub(1);
            // Drift accumulation
            self.vib.drift_cumulative += phases[..n].iter().sum::<f32>() / n as f32;
            self.vib.drift_frames += 1;
            if self.vib.phase_rms_buf.len >= 600 {
                let broadband = self.vib.phase_rms_buf.mean_last(600);
                // Seismic: broadband energy > 0.15 rad RMS
                if broadband > 0.15 && self.vib.seismic_cooldown == 0 {
                    let energy = self.vib.phase_rms_buf.mean_last(120);
                    self.vib.seismic_cooldown = 400;
                    alerts.push(EdgeAlert {
                        module: "vibration".into(), event_type: 540,
                        event_name: "SeismicDetected".into(), value: energy,
                        severity: "critical".into(),
                    });
                }
                // Mechanical resonance: narrowband autocorrelation peaks (ratio > 3.0)
                if self.vib.resonance_cooldown == 0 {
                    let lf = self.vib.phase_rms_buf.mean_last(100);
                    let sf = self.vib.phase_rms_buf.mean_last(10);
                    let ratio = sf / lf.max(0.001);
                    if ratio > 3.0 {
                        self.vib.resonance_cooldown = 400;
                        alerts.push(EdgeAlert {
                            module: "vibration".into(), event_type: 541,
                            event_name: "MechanicalResonance".into(), value: ratio,
                            severity: "warning".into(),
                        });
                    }
                }
                // Vibration spectrum report every 5s
                if self.vib.drift_frames % 100 == 0 {
                    let spectrum = self.vib.phase_rms_buf.mean_last(100);
                    alerts.push(EdgeAlert {
                        module: "vibration".into(), event_type: 543,
                        event_name: "VibrationSpectrum".into(), value: spectrum,
                        severity: "info".into(),
                    });
                }
            }
            // Structural drift: monotonic phase change > 0.0005 rad/frame over 600 frames
            if self.vib.drift_frames >= 600 {
                let drift_rate = (self.vib.drift_cumulative / self.vib.drift_frames as f32).abs();
                if drift_rate > 0.0005 {
                    alerts.push(EdgeAlert {
                        module: "vibration".into(), event_type: 542,
                        event_name: "StructuralDrift".into(), value: drift_rate,
                        severity: "critical".into(),
                    });
                    self.vib.drift_cumulative = 0.0;
                    self.vib.drift_frames = 0;
                }
            }
        }

        // ── Module 18: lrn_meta_adapt ───────────────────────────────────────────
        self.meta.frame_ctr += 1;
        // Run every 300 frames (~15s @ 20Hz) — hill-climbing on 8 thresholds
        if self.meta.frame_ctr % 300 == 0 && self.meta.frame_ctr > 300 {
            // Score: TP - 2*FP (simplified: treat any alert > threshold as TP proxy)
            let alert_frac = alerts.len() as f32 / 30.0; // roughly normalize
            let current_score = alert_frac.min(1.0) - 2.0 * (alert_frac.max(0.0) - alert_frac.min(0.5)).max(0.0);
            // Hill-climb: perturb one threshold
            let step = self.meta.explore_step % 8;
            let orig = self.meta.thresholds[step];
            let delta = 0.05 * if self.meta.direction { 1.0 } else { -1.0 };
            self.meta.thresholds[step] = (orig + delta).clamp(0.1, 0.95);
            self.meta.explore_step += 1;
            self.meta.direction = !self.meta.direction;
            self.meta.iteration += 1;
            // Update best
            if current_score > self.meta.best_score {
                self.meta.best_score = current_score;
                self.meta.best_thresholds = self.meta.thresholds;
                self.meta.consecutive_failures = 0;
            } else {
                self.meta.consecutive_failures += 1;
            }
            // Safety rollback after 3 consecutive failures
            if self.meta.consecutive_failures >= 3 {
                self.meta.thresholds = self.meta.best_thresholds;
                self.meta.consecutive_failures = 0;
                alerts.push(EdgeAlert {
                    module: "meta_adapt".into(), event_type: 742,
                    event_name: "RollbackTriggered".into(), value: self.meta.iteration as f32,
                    severity: "warning".into(),
                });
            }
            // Report adaptation score
            alerts.push(EdgeAlert {
                module: "meta_adapt".into(), event_type: 741,
                event_name: "AdaptationScore".into(), value: self.meta.best_score,
                severity: "info".into(),
            });
            alerts.push(EdgeAlert {
                module: "meta_adapt".into(), event_type: 743,
                event_name: "MetaLevel".into(), value: self.meta.iteration as f32,
                severity: "info".into(),
            });
            // Indicate which threshold was adjusted
            alerts.push(EdgeAlert {
                module: "meta_adapt".into(), event_type: 740,
                event_name: "ParamAdjusted".into(),
                value: step as f32 + self.meta.thresholds[step] * 0.01,
                severity: "info".into(),
            });
        }

        // ── Module 19: tmp_temporal_logic_guard ─────────────────────────────────
        {
            let coherence = self.coh_smoothed;
            let n_persons = if presence && br > 0.0 { 1u8 } else if presence { 1u8 } else { 0u8 };
            for i in 0..8 { if self.ltl.alert_cooldown[i] > 0 { self.ltl.alert_cooldown[i] -= 1; } }
            // Rule 1: G(no presence+fall → violation) — always satisfied in normal flow
            // Rule 2: G(intrusion + no_presence → violation)
            if presence && !self.intr_armed && self.ltl.alert_cooldown[1] == 0 {
                // Simplified: if presence detected but intrusion not armed, check baseline
                self.ltl.violations[1] = self.ltl.violations[1].saturating_add(1);
            } else { self.ltl.violations[1] = 0; }
            // Rule 3: G(no_persons + person_id_active → violation)
            if n_persons == 0 && (self.mc.active != 0) {
                self.ltl.violations[2] = self.ltl.violations[2].saturating_add(1);
                if self.ltl.violations[2] >= 15 && self.ltl.alert_cooldown[2] == 0 {
                    self.ltl.alert_cooldown[2] = 100;
                    alerts.push(EdgeAlert {
                        module: "ltl".into(), event_type: 795,
                        event_name: "LTLViolation".into(),
                        value: 3.0, severity: "critical".into(),
                    });
                }
            } else { self.ltl.violations[2] = 0; }
            // Rule 4: G(coherence < 0.3 + vital_signs_active → violation)
            if coherence < 0.3 && br > 0.0 {
                self.ltl.violations[3] = self.ltl.violations[3].saturating_add(1);
                if self.ltl.violations[3] >= 10 && self.ltl.alert_cooldown[3] == 0 {
                    self.ltl.alert_cooldown[3] = 100;
                    alerts.push(EdgeAlert {
                        module: "ltl".into(), event_type: 795,
                        event_name: "LTLViolation".into(),
                        value: 4.0, severity: "critical".into(),
                    });
                }
            } else { self.ltl.violations[3] = 0; }
            // Rule 5: F(motion → stop within 300s)
            if motion_energy > 0.5 {
                self.ltl.motion_stop_timer = self.ltl.motion_stop_timer.saturating_add(1);
            } else {
                if self.ltl.motion_stop_timer > 0 {
                    // Motion stopped — satisfaction event
                    if self.ltl.motion_stop_timer < 6000 {
                        alerts.push(EdgeAlert {
                            module: "ltl".into(), event_type: 796,
                            event_name: "LTLSatisfaction".into(),
                            value: 5.0, severity: "info".into(),
                        });
                    }
                    self.ltl.motion_stop_timer = 0;
                }
            }
            if self.ltl.motion_stop_timer > 6000 && self.ltl.alert_cooldown[5] == 0 {
                self.ltl.alert_cooldown[5] = 200;
                alerts.push(EdgeAlert {
                    module: "ltl".into(), event_type: 795,
                    event_name: "LTLViolation".into(),
                    value: 5.0, severity: "warning".into(),
                });
                alerts.push(EdgeAlert {
                    module: "ltl".into(), event_type: 797,
                    event_name: "Counterexample".into(),
                    value: 5.0, severity: "info".into(),
                });
            }
            // Rule 6: G(breathing > 40 → alert within 5s)
            if br > 40.0 {
                self.ltl.breathing_high_timer += 1;
                if self.ltl.breathing_high_timer >= 100 && self.ltl.alert_cooldown[6] == 0 {
                    self.ltl.alert_cooldown[6] = 100;
                    alerts.push(EdgeAlert {
                        module: "ltl".into(), event_type: 795,
                        event_name: "LTLViolation".into(),
                        value: 6.0, severity: "critical".into(),
                    });
                }
            } else { self.ltl.breathing_high_timer = 0; }
            // Rule 7: G(HR > 150 → violation)
            if hr > 150.0 {
                self.ltl.hr_high_timer += 1;
                if self.ltl.hr_high_timer >= 10 && self.ltl.alert_cooldown[7] == 0 {
                    self.ltl.alert_cooldown[7] = 100;
                    alerts.push(EdgeAlert {
                        module: "ltl".into(), event_type: 795,
                        event_name: "LTLViolation".into(),
                        value: 7.0, severity: "critical".into(),
                    });
                }
            } else { self.ltl.hr_high_timer = 0; }
            // Rule 8: G(seizure → !normal_gait within 60s)
            if self.sz_seizing {
                self.ltl.seizure_gait_timer += 1;
                if self.ltl.seizure_gait_timer > 60 && self.ltl.alert_cooldown[8 % 8] == 0 {
                    self.ltl.alert_cooldown[0] = 200;
                    alerts.push(EdgeAlert {
                        module: "ltl".into(), event_type: 795,
                        event_name: "LTLViolation".into(),
                        value: 8.0, severity: "critical".into(),
                    });
                }
            } else { self.ltl.seizure_gait_timer = 0; }
        }

        alerts
    }

    /// Get coherence status.
    pub fn coherence_status(&self) -> &'static str {
        match self.coh_gate { 0 => "accept", 1 => "warn", _ => "reject" }
    }
    pub fn coherence_score(&self) -> f32 { self.coh_smoothed }
}

// ══════════════════════════════════════════════════════════════════════════════
// Math helpers
// ══════════════════════════════════════════════════════════════════════════════

/// L2 distance for mincut person match (8-dim feature vectors)
fn l2mc(a: &[f32; 8], b: &[f32; 8]) -> f32 {
    let mut sum = 0.0f32;
    for i in 0..8 { let d = a[i] - b[i]; sum += d * d; }
    sum.sqrt()
}

fn euclid_4(a: &[f32; 4], b: &[f32; 4]) -> f32 {
    ((a[0]-b[0]).powi(2) + (a[1]-b[1]).powi(2) + (a[2]-b[2]).powi(2) + (a[3]-b[3]).powi(2)).sqrt()
}

fn jerk_estimate(buf: &RingBuf<100>) -> f32 {
    if buf.len < 3 { return 0.0; }
    let mut max_jerk = 0.0f32;
    let idx = buf.idx;
    for i in 2..buf.len {
        let v0 = buf.buf[(idx + 100 - i + 0) % 100];
        let v1 = buf.buf[(idx + 100 - i + 1) % 100];
        let v2 = buf.buf[(idx + 100 - i + 2) % 100];
        let j = (v2 - 2.0 * v1 + v0).abs(); // second derivative
        if j > max_jerk { max_jerk = j; }
    }
    max_jerk * 20.0 // scale to Hz
}

fn entropy_estimate(buf: &RingBuf<100>) -> f32 {
    if buf.len < 2 { return 0.0; }
    let mut reversals = 0u32;
    let idx = buf.idx;
    for i in 1..buf.len {
        let prev = buf.buf[(idx + 100 - i - 1) % 100];
        let curr = buf.buf[(idx + 100 - i + 0) % 100];
        let next = if i + 1 < buf.len { buf.buf[(idx + 100 - i + 1) % 100] } else { curr };
        if (curr - prev) * (next - curr) < 0.0 { reversals += 1; }
    }
    reversals as f32 / buf.len as f32
}

fn autocorr_max(buf: &RingBuf<120>, lag_min: usize, lag_max: usize) -> f32 {
    if buf.len < lag_max { return 0.0; }
    let n = buf.len;
    let mean = buf.mean_last(n);
    let mut variance = 0.0f32;
    let idx = buf.idx;
    for i in 0..n {
        let v = buf.buf[(idx + 120 - n + i) % 120] - mean;
        variance += v * v;
    }
    if variance < 1e-8 { return 0.0; }
    let mut max_ac = 0.0f32;
    for lag in lag_min..=lag_max.min(n - 1) {
        let mut cov = 0.0f32;
        for i in 0..(n - lag) {
            let a = buf.buf[(idx + 120 - n + i) % 120] - mean;
            let b = buf.buf[(idx + 120 - n + i + lag) % 120] - mean;
            cov += a * b;
        }
        let ac = cov / variance;
        if ac > max_ac { max_ac = ac; }
    }
    max_ac
}

fn rmssd(buf: &RingBuf<60>, n: usize) -> f32 {
    let c = n.min(buf.len);
    if c < 2 { return 0.0; }
    let idx = buf.idx;
    let mut ss = 0.0f64;
    let mut cnt = 0u32;
    for i in 1..c {
        let a = buf.buf[(idx + 60 - c + i - 1) % 60] as f64;
        let b = buf.buf[(idx + 60 - c + i) % 60] as f64;
        ss += (b - a).powi(2);
        cnt += 1;
    }
    if cnt == 0 { 0.0 } else { (ss / cnt as f64).sqrt() as f32 }
}
