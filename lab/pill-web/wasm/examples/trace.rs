//! Native run of the same C ABI the browser calls, for `parity/parity.ts`.
//!
//! stdin, one command per line: `opts <17 numbers>`, `at <t> <kind> <mode> <flag> <iphone> <n>`,
//! `run <seconds> <dt>`. stdout: one line per frame, the `pill_frame` buffer as numbers.

use std::io::{BufRead, Write};

use pill_wasm::*;

fn main() {
    let mut steps: Vec<(f32, [u32; 5])> = Vec::new();
    let mut out = std::io::BufWriter::new(std::io::stdout());
    for line in std::io::stdin().lock().lines() {
        let line = line.unwrap();
        let mut it = line.split_whitespace();
        let Some(cmd) = it.next() else { continue };
        let nums: Vec<f32> = it.map(|s| s.parse().unwrap()).collect();
        match cmd {
            "opts" => {
                let n = &nums;
                pill_reset();
                pill_set_opts(
                    n[0] as u32, n[1] as u32, n[2], n[3], n[4], n[5], n[6], n[7], n[8], n[9], n[10],
                    n[11], n[12], n[13], n[14], n[15], n[16],
                );
                steps.clear();
            }
            "at" => steps.push((
                nums[0],
                [nums[1] as u32, nums[2] as u32, nums[3] as u32, nums[4] as u32, nums[5] as u32],
            )),
            "run" => {
                let (secs, dt) = (nums[0], nums[1]);
                let mut t = 0.0_f32;
                let mut i = 0;
                while t < secs {
                    while i < steps.len() && t >= steps[i].0 - 1e-6 {
                        let [k, m, f, p, n] = steps[i].1;
                        pill_set_pres(k, m, f, p, n);
                        i += 1;
                    }
                    pill_advance(dt);
                    t += dt;
                    let f = unsafe { std::slice::from_raw_parts(pill_frame(), FRAME_HEADER + FRAME_ELEM * 14) };
                    let text: Vec<String> = f.iter().map(|v| format!("{v:e}")).collect();
                    writeln!(out, "{}", text.join(" ")).unwrap();
                }
                writeln!(out, "end").unwrap();
            }
            _ => {}
        }
    }
}
