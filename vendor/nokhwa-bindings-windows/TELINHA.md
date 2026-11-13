# Local Media Foundation correction

Based on `nokhwa-bindings-windows` 0.4.6 from crates.io (Apache-2.0).
The upstream source and license are retained; no device API was replaced.

The upstream enumeration and setter discard every frame rate whose denominator
is not 1. This hides valid 29.97/59.94 fps modes, including unreduced ratios such
as 30000/1000. The current-format getter also reads the denominator as the rate.

This patch rounds the ratio only for `CameraFormat` selection and reports it
consistently. The setter still applies the original native `IMFMediaType`, so
fractional device rates and other format attributes are preserved. Invalid
zero ratios are rejected. Regression coverage runs with
`cargo test --locked -p nokhwa-bindings-windows` from the repository root.

The app separately queries each supported pixel format to bypass Nokhwa's
two-format enumeration limit; it does not call the truncated request helper.

Revisit this patch when upstream fixes all three ratio conversions.
