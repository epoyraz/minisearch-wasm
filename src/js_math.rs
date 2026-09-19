//! `Math.log`, bit for bit.
//!
//! BM25 scores are compared with JS MiniSearch's down to the last bit, and the
//! only transcendental function in them is the logarithm of the inverse
//! document frequency. V8 computes `Math.log` with fdlibm's `__ieee754_log`.
//! Rust's `f64::ln` is the platform's `log` natively and, on wasm32, a port of
//! musl's, which sums the final terms in a different order: a correctly rounded
//! result either way, but not always the same one. This is fdlibm's, operation
//! for operation; every step is an IEEE 754 double operation, so the result
//! does not depend on the target. The constants are given by their bits, as
//! fdlibm documents them.
//
// ====================================================
// Copyright (C) 1993 by Sun Microsystems, Inc. All rights reserved.
//
// Developed at SunSoft, a Sun Microsystems, Inc. business.
// Permission to use, copy, modify, and distribute this
// software is freely granted, provided that this notice
// is preserved.
// ====================================================

const LN2_HI: f64 = f64::from_bits(0x3fe62e42_fee00000);
const LN2_LO: f64 = f64::from_bits(0x3dea39ef_35793c76);
const TWO54: f64 = f64::from_bits(0x43500000_00000000);
const LG1: f64 = f64::from_bits(0x3fe55555_55555593);
const LG2: f64 = f64::from_bits(0x3fd99999_9997fa04);
const LG3: f64 = f64::from_bits(0x3fd24924_94229359);
const LG4: f64 = f64::from_bits(0x3fcc71c5_1d8e78af);
const LG5: f64 = f64::from_bits(0x3fc74664_96cb03de);
const LG6: f64 = f64::from_bits(0x3fc39a09_d078c69f);
const LG7: f64 = f64::from_bits(0x3fc2f112_df3e5244);

/// Natural logarithm as JavaScript's `Math.log` computes it.
pub fn log(mut x: f64) -> f64 {
    let mut hx = (x.to_bits() >> 32) as i32;
    let lx = x.to_bits() as u32;

    let mut k: i32 = 0;
    if hx < 0x0010_0000 {
        // x < 2**-1022
        if ((hx & 0x7fff_ffff) as u32 | lx) == 0 {
            return f64::NEG_INFINITY; // log(+-0) = -inf
        }
        if hx < 0 {
            return f64::NAN; // log(-#) = NaN
        }
        // Subnormal number: scale up x.
        k -= 54;
        x *= TWO54;
        hx = (x.to_bits() >> 32) as i32;
    }
    if hx >= 0x7ff0_0000 {
        return x + x;
    }
    k += (hx >> 20) - 1023;
    hx &= 0x000f_ffff;
    let mut i = (hx + 0x95f64) & 0x10_0000;
    // Normalize x or x/2.
    x = f64::from_bits(
        (((hx | (i ^ 0x3ff0_0000)) as u32 as u64) << 32) | (x.to_bits() & 0xffff_ffff),
    );
    k += i >> 20;
    let f = x - 1.0;
    let dk = k as f64;
    if (0x000f_ffff & (2 + hx)) < 3 {
        // |f| < 2**-20
        if f == 0.0 {
            return if k == 0 {
                0.0
            } else {
                dk * LN2_HI + dk * LN2_LO
            };
        }
        let r = f * f * (0.5 - (1.0 / 3.0) * f);
        return if k == 0 {
            f - r
        } else {
            dk * LN2_HI - ((r - dk * LN2_LO) - f)
        };
    }
    let s = f / (2.0 + f);
    let z = s * s;
    i = hx - 0x6147a;
    let w = z * z;
    let j = 0x6b851 - hx;
    let t1 = w * (LG2 + w * (LG4 + w * LG6));
    let t2 = z * (LG1 + w * (LG3 + w * (LG5 + w * LG7)));
    i |= j;
    let r = t2 + t1;
    if i > 0 {
        let hfsq = 0.5 * f * f;
        if k == 0 {
            f - (hfsq - s * (hfsq + r))
        } else {
            dk * LN2_HI - ((hfsq - (s * (hfsq + r) + dk * LN2_LO)) - f)
        }
    } else if k == 0 {
        f - s * (f - r)
    } else {
        dk * LN2_HI - ((s * (f - r) - dk * LN2_LO) - f)
    }
}
