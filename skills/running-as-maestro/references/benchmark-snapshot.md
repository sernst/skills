# Model benchmark snapshot

Generated supporting evidence for maestro model/effort selection. Compare only
within a source and version; task-specific judgment and the current roster remain
authoritative. `★` marks the point-estimate cost/performance Pareto frontier.

- Retrieved after semantic change: `2026-09-11T13:34:05Z`
- Parser version: `6`
- Normalized SHA-256: `644351be08660fe73bac1d3c61b9fd5a1d16a998bdb4835f7c0d76bd2a807b22`
- Scores and costs are source-reported; no composite or cross-source ranking is calculated.

## DeepSWE

Source: [DeepSWE](https://deepswe.datacurve.ai/) · version `1.1` · source updated `2026-09-03T22:24:37Z` · tasks `113` · normalized SHA-256 `f2705f16933f703765010c9f0313a2bd0165444c070de3fb9fec204fe98e4a9d`

Metric: `pass@1` · Autonomous software-engineering rollouts; score is attempt pass@1. Confidence intervals are run-to-run estimates; provider, verifier, and network errors are excluded.

Shared harness: `mini-swe-agent` · configuration is derived from model + effort.

| model | effort | score | avg cost/task | uncertainty / sample | Pareto |
| --- | --- | ---: | ---: | --- | :---: |
| gpt-6-astra | xhigh | 74.12% | $6.524 | 95% CI 71.25–76.98%; n=452; runs=4 | ★ |
| gemini-3-8-flash | high | 73.83% | $2.362 | 95% CI 72.41–75.24%; n=447; runs=4 | ★ |
| claude-opus-5 | max | 73.65% | $11.838 | 95% CI 69.78–77.52%; n=444; runs=4 |  |
| gpt-6-astra | high | 73.23% | $5.724 | 95% CI 69.81–76.65%; n=452; runs=4 |  |
| gpt-6-astra | max | 73.23% | $12.369 | 95% CI 72.40–74.06%; n=452; runs=4 |  |
| claude-opus-5 | xhigh | 73.15% | $9.072 | 95% CI 70.09–76.22%; n=447; runs=4 |  |
| claude-opus-5 | high | 72.83% | $6.076 | 95% CI 70.88–74.77%; n=449; runs=4 |  |
| gpt-6-astra | medium | 72.79% | $4.380 | 95% CI 70.20–75.38%; n=452; runs=4 |  |
| gpt-5-6-sol | max | 72.67% | $8.386 | 95% CI 69.84–75.50%; n=450; runs=4 |  |
| gemini-3-8-flash | medium | 71.02% | $1.967 | 95% CI 68.74–73.30%; n=452; runs=4 | ★ |
| gpt-5-6-sol | xhigh | 70.73% | $4.704 | 95% CI 69.91–71.55%; n=451; runs=4 |  |
| claude-fable-5 | xhigh | 69.91% | $13.415 | 95% CI 66.67–73.16%; n=452; runs=4 |  |
| claude-fable-5 | max | 69.72% | $21.635 | 95% CI 65.69–73.76%; n=436; runs=4 |  |
| gpt-5-6-terra | max | 69.62% | $4.946 | 95% CI 67.07–72.18%; n=451; runs=4 |  |
| gpt-5-6-sol | high | 69.40% | $3.470 | 95% CI 67.97–70.83%; n=451; runs=4 |  |
| glm-5-3 | max | 68.96% | $3.993 | 95% CI 65.94–71.98%; n=451; runs=4 |  |
| claude-opus-5 | medium | 68.90% | $3.290 | 95% CI 67.73–70.08%; n=447; runs=4 |  |
| claude-fable-5 | high | 68.60% | $9.178 | 95% CI 67.48–69.73%; n=430; runs=4 |  |
| kimi-k3 | max | 68.51% | $4.655 | 95% CI 63.98–73.05%; n=451; runs=4 |  |
| grok-4-6 | medium | 67.48% | $3.449 | 95% CI 65.20–69.76%; n=452; runs=4 |  |
| gpt-5-6-luna | max | 67.19% | $3.028 | 95% CI 63.20–71.18%; n=448; runs=4 |  |
| gpt-6-astra | low | 67.04% | $2.189 | 95% CI 65.73–68.34%; n=452; runs=4 |  |
| gpt-5-5 | xhigh | 67.04% | $7.226 | 95% CI 60.57–73.50%; n=452; runs=4 |  |
| grok-4-6 | xhigh | 66.74% | $5.498 | 95% CI 64.56–68.92%; n=451; runs=4 |  |
| gemini-3-7-flash | medium | 65.49% | $2.025 | 95% CI 62.40–68.57%; n=452; runs=4 |  |
| claude-fable-5 | medium | 65.37% | $6.088 | 95% CI 60.95–69.79%; n=436; runs=4 |  |
| gemini-3-7-flash | high | 65.27% | $2.176 | 95% CI 63.48–67.05%; n=452; runs=4 |  |
| grok-4-6 | high | 65.19% | $4.385 | 95% CI 63.65–66.72%; n=451; runs=4 |  |
| gpt-5-5 | high | 64.38% | $5.100 | 95% CI 61.26–67.50%; n=452; runs=4 |  |
| glm-5-3-flash | max | 63.39% | $0.482 | 95% CI 59.01–67.77%; n=448; runs=4 | ★ |
| deepseek-v4-pro | max | 62.83% | $0.241 | 95% CI 56.50–69.17%; n=452; runs=4 | ★ |
| gpt-5-6-sol | medium | 61.06% | $1.862 | 95% CI 59.48–62.65%; n=452; runs=4 |  |
| gpt-5-6-terra | xhigh | 60.18% | $2.127 | 95% CI 58.05–62.30%; n=452; runs=4 |  |
| claude-fable-5 | low | 59.58% | $3.758 | 95% CI 56.79–62.38%; n=433; runs=4 |  |
| claude-opus-4-8 | max | 58.97% | $13.223 | 95% CI 57.21–60.74%; n=429; runs=4 |  |
| claude-opus-5 | low | 58.13% | $1.663 | 95% CI 55.80–60.46%; n=449; runs=4 |  |
| qwen3-8-max | xhigh | 57.46% | $3.729 | 95% CI 54.80–60.12%; n=449; runs=4 |  |
| gpt-5-6-luna | xhigh | 56.86% | $1.536 | 95% CI 54.69–59.03%; n=452; runs=4 |  |
| muse-spark-1-2 | xhigh | 54.87% | $3.696 | 95% CI 52.74–56.99%; n=452; runs=4 |  |
| claude-opus-4-8 | xhigh | 54.36% | $8.006 | 95% CI 50.65–58.08%; n=447; runs=4 |  |
| gpt-5-5 | medium | 53.98% | $2.749 | 95% CI 51.43–56.54%; n=452; runs=4 |  |
| claude-sonnet-5 | max | 53.85% | $26.400 | 95% CI 49.61–58.08%; n=442; runs=4 |  |
| gpt-5-6-terra | high | 53.76% | $1.134 | 95% CI 49.43–58.09%; n=452; runs=4 |  |
| gemini-3-7-flash | low | 53.76% | $1.832 | 95% CI 51.17–56.35%; n=452; runs=4 |  |
| grok-4-5 | high | 53.76% | $2.416 | 95% CI 51.48–56.04%; n=452; runs=4 |  |
| deepseek-v4-flash | max | 53.32% | $0.100 | 95% CI 49.75–56.89%; n=452; runs=4 | ★ |
| muse-spark-1-1 | xhigh | 53.32% | $2.361 | 95% CI 50.28–56.35%; n=452; runs=4 |  |
| claude-opus-4-8 | high | 51.77% | $4.282 | 95% CI 47.21–56.33%; n=452; runs=4 |  |
| gpt-5-4 | xhigh | 51.77% | $5.652 | 95% CI 50.27–53.27%; n=452; runs=4 |  |
| claude-sonnet-5 | xhigh | 49.67% | $11.891 | 95% CI 46.21–53.12%; n=451; runs=4 |  |
| claude-opus-4-8 | medium | 48.67% | $3.444 | 95% CI 46.43–50.91%; n=452; runs=4 |  |
| claude-sonnet-5 | high | 48.23% | $7.426 | 95% CI 43.72–52.74%; n=452; runs=4 |  |
| gemini-3-6-flash | high | 46.68% | $4.419 | 95% CI 42.98–50.39%; n=452; runs=4 |  |
| gpt-5-6-sol | low | 45.35% | $1.074 | 95% CI 42.97–47.74%; n=452; runs=4 |  |
| gpt-5-6-luna | high | 44.25% | $0.778 | 95% CI 41.33–47.17%; n=452; runs=4 |  |
| glm-5-2 | max | 43.78% | $3.920 | 95% CI 42.05–45.50%; n=450; runs=4 |  |
| grok-4-6 | low | 41.65% | $1.042 | 95% CI 39.33–43.97%; n=449; runs=4 |  |
| claude-opus-4-8 | low | 40.80% | $2.293 | 95% CI 39.33–42.26%; n=451; runs=4 |  |
| claude-sonnet-5 | medium | 39.78% | $4.079 | 95% CI 36.65–42.91%; n=450; runs=4 |  |
| glm-5-2 | high | 36.28% | $2.836 | 95% CI 31.53–41.03%; n=452; runs=4 |  |
| gemini-3-5-flash | high | 36.06% | $3.447 | 95% CI 32.10–40.03%; n=452; runs=4 |  |
| gpt-5-6-terra | medium | 35.11% | $0.583 | 95% CI 31.73–38.49%; n=450; runs=4 |  |
| kimi-k2-7-code | default | 30.53% | $2.816 | 95% CI 30.03–31.03%; n=452; runs=4 |  |
| claude-sonnet-5 | low | 30.51% | $2.187 | 95% CI 29.39–31.64%; n=449; runs=4 |  |
| claude-sonnet-4-6 | high | 29.93% | $5.522 | 95% CI 25.84–34.03%; n=451; runs=4 |  |
| gpt-5-5 | low | 26.99% | $1.200 | 95% CI 24.70–29.29%; n=452; runs=4 |  |
| gpt-5-6-terra | low | 24.05% | $0.428 | 95% CI 23.28–24.83%; n=449; runs=4 |  |
| gemini-3-1-pro-preview | high | 11.73% | $2.143 | 95% CI 10.24–13.21%; n=452; runs=4 |  |
| gpt-5-6-luna | medium | 11.28% | $0.216 | 95% CI 10.45–12.11%; n=452; runs=4 |  |
| gpt-5-6-luna | low | 1.55% | $0.072 | 95% CI 0.72–2.38%; n=452; runs=4 | ★ |

## CursorBench

Source: [CursorBench](https://cursor.com/cursorbench) · version `4.0` · source updated `2026-09-10` · normalized SHA-256 `08acbd0f50323e0f691979cb91d82f9a50326c5d5ac06d0fb55fe639efca9beb`

Metric: `score` · Ambiguous, multi-file tasks from real Cursor sessions. No uncertainty or sample count is published; small score differences may not be meaningful.

Shared harness/config: `Cursor benchmark agent` · `published CursorBench configuration`.

| model | effort | score | avg cost/task | uncertainty / sample | Pareto |
| --- | --- | ---: | ---: | --- | :---: |
| Fable 5.1 | Max | 51.80% | $17.280 | — | ★ |
| Fable 5.1 | Extra High | 51.60% | $13.010 | — | ★ |
| Fable 5.1 | High | 49.20% | $9.080 | — | ★ |
| Fable 5.1 | Medium | 46.80% | $7.050 | — | ★ |
| Opus 5 | Max | 46.60% | $11.950 | — |  |
| Opus 5 | Extra High | 46.10% | $11.430 | — |  |
| Fable 5.1 | Low | 45.10% | $5.440 | — | ★ |
| Opus 5 | High | 44.70% | $9.000 | — |  |
| Opus 5 | Medium | 43.30% | $6.940 | — |  |
| GPT-5.6 Sol | Max | 41.70% | $8.230 | — |  |
| Muse Spark 1.3 | Max | 41.60% | $2.640 | — | ★ |
| Grok 4.6 | Extra High | 41.40% | $6.100 | — |  |
| GPT-5.6 Terra | Max | 41.30% | $5.140 | — |  |
| Opus 5 | Low | 40.70% | $4.870 | — |  |
| Grok 4.6 | High | 40.40% | $5.200 | — |  |
| Gemini 3.8 Flash | High | 39.60% | $4.700 | — |  |
| GPT-5.6 Sol | Extra High | 37.70% | $4.400 | — |  |
| Muse Spark 1.3 | Extra High | 37.50% | $2.100 | — | ★ |
| Gemini 3.8 Flash | Medium | 37.30% | $4.060 | — |  |
| Grok 4.6 | Medium | 36.10% | $3.480 | — |  |
| GPT-5.6 Luna | Max | 35.90% | $1.030 | — | ★ |
| GPT-5.6 Sol | High | 35.70% | $2.850 | — |  |
| Sonnet 5 | Max | 34.10% | $7.170 | — |  |
| GPT-5.6 Terra | Extra High | 33.60% | $1.810 | — |  |
| Muse Spark 1.3 | High | 33.40% | $1.660 | — |  |
| Grok 4.6 | Low | 33.40% | $2.250 | — |  |
| GPT-5.6 Luna | Extra High | 33.00% | $0.440 | — | ★ |
| Muse Spark 1.3 | Medium | 32.60% | $1.490 | — |  |
| Sonnet 5 | Extra High | 32.00% | $4.550 | — |  |
| GPT-5.6 Sol | Medium | 31.10% | $1.770 | — |  |
| Sonnet 5 | High | 30.80% | $3.480 | — |  |
| GPT-5.6 Terra | High | 30.70% | $1.110 | — |  |
| GPT-5.6 Luna | High | 29.40% | $0.250 | — | ★ |
| Muse Spark 1.3 | Low | 29.30% | $0.930 | — |  |
| Sonnet 5 | Medium | 28.00% | $2.310 | — |  |
| Composer 2.5 | default | 27.70% | $0.680 | — |  |
| GPT-5.6 Terra | Medium | 27.60% | $0.640 | — |  |
| GPT-5.6 Terra | Low | 25.20% | $0.520 | — |  |
| GPT-5.6 Sol | Low | 24.60% | $0.870 | — |  |
| Muse Spark 1.3 | Minimal | 24.30% | $0.560 | — |  |
| Sonnet 5 | Low | 24.10% | $1.390 | — |  |
| GPT-5.6 Luna | Medium | 22.20% | $0.080 | — | ★ |
| GPT-5.6 Luna | Low | 16.00% | $0.030 | — | ★ |
