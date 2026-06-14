//! Fixed-capacity ring buffers of recent samples, fed once per refresh, drawn
//! by the graph primitive. All series are normalized to 0..1.

use crate::collect::Host;
use std::collections::VecDeque;

const CAP: usize = 240;

#[derive(Default)]
pub struct Series(VecDeque<f64>);

impl Series {
    fn push(&mut self, v: f64) {
        if self.0.len() == CAP {
            self.0.pop_front();
        }
        self.0.push_back(v.clamp(0.0, 1.0));
    }
    pub fn slice(&self) -> Vec<f64> {
        self.0.iter().copied().collect()
    }
}

#[derive(Default)]
pub struct History {
    pub pve_mem: Series,
    pub ai_mem: Series,
    pub pve_cpu: Series,
    pub ai_cpu: Series,
    pub pve_temp: Series,
    pub ai_temp: Series,
    pub pve_power: Series, // CPU package power as a fraction of its cap
    pub gpu_mem: Series,
    pub gpu_util: Series,     // SM / graphics utilization
    pub gpu_mem_util: Series, // memory-controller utilization
    pub gpu_power: Series,    // power draw as a fraction of the limit
    pub gpu_temp: Series,     // GPU core temperature (/100)
}

impl History {
    pub fn record(&mut self, pve: &Host, ai: &Host) {
        self.pve_mem.push(pve.mem.frac());
        self.ai_mem.push(ai.mem.frac());
        self.pve_cpu.push(pve.cpu.overall);
        self.ai_cpu.push(ai.cpu.overall);
        self.pve_temp.push(pve.max_temp() / 100.0);
        self.ai_temp.push(ai.max_temp() / 100.0);
        self.pve_power.push(pve.power.frac());
        if let Some(g) = ai.gpus.first() {
            self.gpu_mem.push(g.frac());
            self.gpu_util.push(g.util as f64 / 100.0);
            self.gpu_mem_util.push(g.mem_util as f64 / 100.0);
            self.gpu_power.push(g.power_frac());
            self.gpu_temp.push(g.temp_c / 100.0);
        } else {
            self.gpu_mem.push(0.0);
            self.gpu_util.push(0.0);
            self.gpu_mem_util.push(0.0);
            self.gpu_power.push(0.0);
            self.gpu_temp.push(0.0);
        }
    }
}
