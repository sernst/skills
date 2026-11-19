# Model benchmark snapshot

Generated supporting evidence for maestro model/effort selection. Compare only
within a source and version; task-specific judgment and the current roster remain
authoritative. `★` marks the point-estimate cost/performance Pareto frontier.

- Retrieved after semantic change: `2026-11-19T18:15:57Z`
- Parser version: `6`
- Normalized SHA-256: `4322efd2c0a2d8b1577b0c1fec99fb624da507f7c883a71e9ff98f009bf6010a`
- Scores and costs are source-reported; no composite or cross-source ranking is calculated.

## DeepSWE

Source: [DeepSWE](https://deepswe.datacurve.ai/) · version `1.1` · source updated `2026-09-22T06:27:15Z` · tasks `113` · normalized SHA-256 `805f99dc7170a64143cc6145e7b437e6e212caf9cd9b0a2e3d47f144728e6b34`

Metric: `pass@1` · Autonomous software-engineering rollouts; score is attempt pass@1. Confidence intervals are run-to-run estimates; provider, verifier, and network errors are excluded.

Shared harness: `mini-swe-agent` · configuration is derived from model + effort.

| model | effort | score | avg cost/task | uncertainty / sample | Pareto |
| --- | --- | ---: | ---: | --- | :---: |
| gpt-6-astra | xhigh | 74.12% | $4.429 | 95% CI 71.25–76.98%; n=452; runs=4 | ★ |
| gemini-3-8-flash | high | 73.83% | $2.362 | 95% CI 72.41–75.24%; n=447; runs=4 | ★ |
| claude-opus-5 | max | 73.65% | $11.838 | 95% CI 69.78–77.52%; n=444; runs=4 |  |
| gpt-6-astra | high | 73.23% | $3.924 | 95% CI 69.81–76.65%; n=452; runs=4 |  |
| gpt-6-astra | max | 73.23% | $7.498 | 95% CI 72.40–74.06%; n=452; runs=4 |  |
| claude-opus-5 | xhigh | 73.15% | $9.072 | 95% CI 70.09–76.22%; n=447; runs=4 |  |
| claude-opus-5 | high | 72.83% | $6.076 | 95% CI 70.88–74.77%; n=449; runs=4 |  |
| gpt-6-astra | medium | 72.79% | $3.075 | 95% CI 70.20–75.38%; n=452; runs=4 |  |
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
| gpt-6-astra | low | 67.04% | $1.595 | 95% CI 65.73–68.34%; n=452; runs=4 | ★ |
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

Source: [CursorBench](https://cursor.com/cursorbench) · version `4.1` · source updated `2026-11-18` · normalized SHA-256 `cb63c22284dfe106a12538fff8f3c1b602e8ac48bf74549245ff1f0f9c301b5e`

Metric: `score` · Ambiguous, multi-file tasks from real Cursor sessions. No uncertainty or sample count is published; small score differences may not be meaningful.

Shared harness/config: `Cursor benchmark agent` · `published CursorBench configuration`.

| model | effort | score | avg cost/task | uncertainty / sample | Pareto |
| --- | --- | ---: | ---: | --- | :---: |
| Gemini 4 Neon | Extra High | 65.40% | $10.670 | — | ★ |
| Gemini 4 Neon | High | 63.70% | $6.540 | — | ★ |
| Gemini 4 Carbon | High | 61.50% | $8.520 | — |  |
| Gemini 4 Hydrogen | Extra High | 61.40% | $8.090 | — |  |
| Fable 5.2 | Max | 61.30% | $48.110 | — |  |
| Gemini 4 Carbon | Medium | 60.40% | $7.220 | — |  |
| Fable 5.2 | Extra High | 58.80% | $17.840 | — |  |
| Opus 5.5 | Max | 58.10% | $11.880 | — |  |
| Gemini 4 Neon | Medium | 57.70% | $3.070 | — | ★ |
| Gemini 4 Hydrogen | High | 57.30% | $4.570 | — |  |
| Opus 5.5 | Extra High | 56.70% | $6.540 | — |  |
| Gemini 4 Argon | High | 56.50% | $9.140 | — |  |
| Fable 5.2 | High | 56.50% | $10.510 | — |  |
| Gemini 4 Argon | Medium | 55.80% | $7.640 | — |  |
| Gemini 4 Carbon | Low | 55.00% | $8.640 | — |  |
| Opus 5.5 | High | 54.50% | $3.870 | — |  |
| Sonnet 5.5 | Max | 54.40% | $8.630 | — |  |
| Fable 5.2 | Medium | 54.20% | $5.010 | — |  |
| Fable 5.2 | Low | 53.40% | $4.260 | — |  |
| Opus 5.5 | Medium | 50.00% | $2.850 | — | ★ |
| Sonnet 5.5 | Extra High | 49.90% | $3.130 | — |  |
| Grok 4.7 | Extra High | 48.60% | $10.190 | — |  |
| Haiku 5.5 | Max | 48.40% | $1.140 | — | ★ |
| Gemini 4 Argon | Low | 47.90% | $7.960 | — |  |
| Gemini 4 Hydrogen | Medium | 46.40% | $1.280 | — |  |
| Grok 4.7 | High | 46.00% | $6.880 | — |  |
| Gemini 4 Neon | Low | 45.50% | $1.260 | — |  |
| Haiku 5.5 | Extra High | 45.20% | $0.560 | — | ★ |
| Grok 4.6 | Extra High | 44.30% | $5.260 | — |  |
| Sonnet 5.5 | High | 43.80% | $1.410 | — |  |
| Opus 5.5 | Low | 42.70% | $1.430 | — |  |
| Grok 4.7 | Medium | 42.50% | $5.520 | — |  |
| Muse Spark 1.3 | Max | 41.90% | $2.890 | — |  |
| Grok 4.6 | High | 41.30% | $4.280 | — |  |
| GPT-5.6 Sol | Max | 40.90% | $7.600 | — |  |
| Grok 4.6 | Medium | 38.90% | $2.590 | — |  |
| Haiku 5.5 | High | 37.50% | $0.310 | — | ★ |
| GPT-5.6 Sol | Extra High | 37.40% | $4.320 | — |  |
| Muse Spark 1.3 | Extra High | 34.60% | $2.330 | — |  |
| Sonnet 5.5 | Medium | 34.30% | $0.630 | — |  |
| Haiku 5.5 | Medium | 34.20% | $0.170 | — | ★ |
| GPT-5.6 Sol | High | 33.20% | $3.000 | — |  |
| Grok 4.6 | Low | 33.10% | $1.760 | — |  |
| Muse Spark 1.3 | High | 32.10% | $1.970 | — |  |
| Sonnet 5.5 | Low | 31.80% | $0.440 | — |  |
| Grok 4.7 | Low | 31.50% | $2.730 | — |  |
| Muse Spark 1.3 | Medium | 31.30% | $1.750 | — |  |
| GPT-5.6 Sol | Medium | 28.70% | $1.970 | — |  |
| Haiku 5.5 | Low | 27.80% | $0.080 | — | ★ |
| Muse Spark 1.3 | Low | 27.50% | $1.130 | — |  |
| Gemini 4 Hydrogen | Low | 23.90% | $0.280 | — |  |
| GPT-5.6 Sol | Low | 23.00% | $0.980 | — |  |
| Muse Spark 1.3 | Minimal | 21.30% | $0.690 | — |  |
