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
    pub gpu_mem: Series,
    pub gpu_util: Series,
}

impl History {
    pub fn record(&mut self, pve: &Host, ai: &Host) {
        self.pve_mem.push(pve.mem.frac());
        self.ai_mem.push(ai.mem.frac());
        self.pve_cpu.push(pve.cpu.overall);
        self.ai_cpu.push(ai.cpu.overall);
        self.pve_temp.push(pve.max_temp() / 100.0);
        self.ai_temp.push(ai.max_temp() / 100.0);
        if let Some(g) = ai.gpus.first() {
            self.gpu_mem.push(g.frac());
            self.gpu_util.push(g.util as f64 / 100.0);
        } else {
            self.gpu_mem.push(0.0);
            self.gpu_util.push(0.0);
        }
    }
}
