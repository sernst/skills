# Model benchmark snapshot

Generated supporting evidence for maestro model/effort selection. Compare only
within a source and version; task-specific judgment and the current roster remain
authoritative. `★` marks the point-estimate cost/performance Pareto frontier.

- Retrieved after semantic change: `2026-08-24T19:33:24Z`
- Parser version: `3`
- Normalized SHA-256: `a8e87b020b1d635ac68e44bcc8f42b86706b3b46166cb61a78b07d87b358f28b`
- Scores and costs are source-reported; no composite or cross-source ranking is calculated.

## DeepSWE

Source: [DeepSWE](https://deepswe.datacurve.ai/) · version `1.1` · source updated `2026-08-20T07:48:24Z` · tasks `113` · normalized SHA-256 `940a871b80f472beb695e7095bbe9631e6efd6c74a71b9bc8da07ae252b85733`

Metric: `pass@1` · Autonomous software-engineering rollouts; score is attempt pass@1. Confidence intervals are run-to-run estimates; provider, verifier, and network errors are excluded.

| model | effort | harness / config | score | avg cost/task | uncertainty / sample | Pareto |
| --- | --- | --- | ---: | ---: | --- | :---: |
| claude-opus-5 | max | mini-swe-agent &#47; mini\_swe\_agent\_claude\_opus\_5\_max | 73.65% | $11.838 | 95% CI 69.78–77.52%; n=444; runs=4 | ★ |
| claude-opus-5 | xhigh | mini-swe-agent &#47; mini\_swe\_agent\_claude\_opus\_5\_xhigh | 73.15% | $9.072 | 95% CI 70.09–76.22%; n=447; runs=4 | ★ |
| claude-opus-5 | high | mini-swe-agent &#47; mini\_swe\_agent\_claude\_opus\_5\_high | 72.83% | $6.076 | 95% CI 70.88–74.77%; n=449; runs=4 | ★ |
| gpt-5-6-sol | max | mini-swe-agent &#47; mini\_swe\_agent\_gpt\_5\_6\_sol\_max | 72.67% | $8.386 | 95% CI 69.84–75.50%; n=450; runs=4 |  |
| gpt-5-6-sol | xhigh | mini-swe-agent &#47; mini\_swe\_agent\_gpt\_5\_6\_sol\_xhigh | 70.73% | $4.704 | 95% CI 69.91–71.55%; n=451; runs=4 | ★ |
| claude-fable-5 | xhigh | mini-swe-agent &#47; mini\_swe\_agent\_claude\_fable\_5\_xhigh | 69.91% | $13.415 | 95% CI 66.67–73.16%; n=452; runs=4 |  |
| claude-fable-5 | max | mini-swe-agent &#47; mini\_swe\_agent\_claude\_fable\_5\_max | 69.72% | $21.635 | 95% CI 65.69–73.76%; n=436; runs=4 |  |
| gpt-5-6-terra | max | mini-swe-agent &#47; mini\_swe\_agent\_gpt\_5\_6\_terra\_max | 69.62% | $4.946 | 95% CI 67.07–72.18%; n=451; runs=4 |  |
| gpt-5-6-sol | high | mini-swe-agent &#47; mini\_swe\_agent\_gpt\_5\_6\_sol\_high | 69.40% | $3.470 | 95% CI 67.97–70.83%; n=451; runs=4 | ★ |
| glm-5-3 | max | mini-swe-agent &#47; mini\_swe\_agent\_glm\_5\_3\_max | 68.96% | $3.993 | 95% CI 65.94–71.98%; n=451; runs=4 |  |
| claude-opus-5 | medium | mini-swe-agent &#47; mini\_swe\_agent\_claude\_opus\_5\_medium | 68.90% | $3.290 | 95% CI 67.73–70.08%; n=447; runs=4 | ★ |
| claude-fable-5 | high | mini-swe-agent &#47; mini\_swe\_agent\_claude\_fable\_5\_high | 68.60% | $9.178 | 95% CI 67.48–69.73%; n=430; runs=4 |  |
| kimi-k3 | max | mini-swe-agent &#47; mini\_swe\_agent\_kimi\_k3\_max | 68.51% | $4.655 | 95% CI 63.98–73.05%; n=451; runs=4 |  |
| grok-4-6 | medium | mini-swe-agent &#47; mini\_swe\_agent\_grok\_4\_6\_medium | 67.48% | $3.449 | 95% CI 65.20–69.76%; n=452; runs=4 |  |
| gpt-5-6-luna | max | mini-swe-agent &#47; mini\_swe\_agent\_gpt\_5\_6\_luna\_max | 67.19% | $3.028 | 95% CI 63.20–71.18%; n=448; runs=4 | ★ |
| gpt-5-5 | xhigh | mini-swe-agent &#47; mini\_swe\_agent\_gpt\_5\_5\_xhigh | 67.04% | $7.226 | 95% CI 60.57–73.50%; n=452; runs=4 |  |
| grok-4-6 | xhigh | mini-swe-agent &#47; mini\_swe\_agent\_grok\_4\_6\_xhigh | 66.74% | $5.498 | 95% CI 64.56–68.92%; n=451; runs=4 |  |
| gemini-3-7-flash | medium | mini-swe-agent &#47; mini\_swe\_agent\_gemini\_3\_7\_flash\_medium | 65.49% | $2.025 | 95% CI 62.40–68.57%; n=452; runs=4 | ★ |
| claude-fable-5 | medium | mini-swe-agent &#47; mini\_swe\_agent\_claude\_fable\_5\_medium | 65.37% | $6.088 | 95% CI 60.95–69.79%; n=436; runs=4 |  |
| gemini-3-7-flash | high | mini-swe-agent &#47; mini\_swe\_agent\_gemini\_3\_7\_flash\_high | 65.27% | $2.176 | 95% CI 63.48–67.05%; n=452; runs=4 |  |
| grok-4-6 | high | mini-swe-agent &#47; mini\_swe\_agent\_grok\_4\_6\_high | 65.19% | $4.385 | 95% CI 63.65–66.72%; n=451; runs=4 |  |
| gpt-5-5 | high | mini-swe-agent &#47; mini\_swe\_agent\_gpt\_5\_5\_high | 64.38% | $5.100 | 95% CI 61.26–67.50%; n=452; runs=4 |  |
| deepseek-v4-pro | max | mini-swe-agent &#47; mini\_swe\_agent\_deepseek\_v4\_pro\_max | 62.83% | $0.241 | 95% CI 56.50–69.17%; n=452; runs=4 | ★ |
| gpt-5-6-sol | medium | mini-swe-agent &#47; mini\_swe\_agent\_gpt\_5\_6\_sol\_medium | 61.06% | $1.862 | 95% CI 59.48–62.65%; n=452; runs=4 |  |
| gpt-5-6-terra | xhigh | mini-swe-agent &#47; mini\_swe\_agent\_gpt\_5\_6\_terra\_xhigh | 60.18% | $2.127 | 95% CI 58.05–62.30%; n=452; runs=4 |  |
| claude-fable-5 | low | mini-swe-agent &#47; mini\_swe\_agent\_claude\_fable\_5\_low | 59.58% | $3.758 | 95% CI 56.79–62.38%; n=433; runs=4 |  |
| claude-opus-4-8 | max | mini-swe-agent &#47; mini\_swe\_agent\_claude\_opus\_4\_8\_max | 58.97% | $13.223 | 95% CI 57.21–60.74%; n=429; runs=4 |  |
| claude-opus-5 | low | mini-swe-agent &#47; mini\_swe\_agent\_claude\_opus\_5\_low | 58.13% | $1.663 | 95% CI 55.80–60.46%; n=449; runs=4 |  |
| qwen3-8-max | xhigh | mini-swe-agent &#47; mini\_swe\_agent\_qwen3\_8\_max\_xhigh | 57.46% | $3.729 | 95% CI 54.80–60.12%; n=449; runs=4 |  |
| gpt-5-6-luna | xhigh | mini-swe-agent &#47; mini\_swe\_agent\_gpt\_5\_6\_luna\_xhigh | 56.86% | $1.536 | 95% CI 54.69–59.03%; n=452; runs=4 |  |
| muse-spark-1-2 | xhigh | mini-swe-agent &#47; mini\_swe\_agent\_muse\_spark\_1\_2\_xhigh | 54.87% | $3.696 | 95% CI 52.74–56.99%; n=452; runs=4 |  |
| claude-opus-4-8 | xhigh | mini-swe-agent &#47; mini\_swe\_agent\_claude\_opus\_4\_8\_xhigh | 54.36% | $8.006 | 95% CI 50.65–58.08%; n=447; runs=4 |  |
| gpt-5-5 | medium | mini-swe-agent &#47; mini\_swe\_agent\_gpt\_5\_5\_medium | 53.98% | $2.749 | 95% CI 51.43–56.54%; n=452; runs=4 |  |
| claude-sonnet-5 | max | mini-swe-agent &#47; mini\_swe\_agent\_claude\_sonnet\_5\_max | 53.85% | $26.400 | 95% CI 49.61–58.08%; n=442; runs=4 |  |
| gpt-5-6-terra | high | mini-swe-agent &#47; mini\_swe\_agent\_gpt\_5\_6\_terra\_high | 53.76% | $1.134 | 95% CI 49.43–58.09%; n=452; runs=4 |  |
| gemini-3-7-flash | low | mini-swe-agent &#47; mini\_swe\_agent\_gemini\_3\_7\_flash\_low | 53.76% | $1.832 | 95% CI 51.17–56.35%; n=452; runs=4 |  |
| grok-4-5 | high | mini-swe-agent &#47; mini\_swe\_agent\_grok\_4\_5\_high | 53.76% | $2.416 | 95% CI 51.48–56.04%; n=452; runs=4 |  |
| deepseek-v4-flash | max | mini-swe-agent &#47; mini\_swe\_agent\_deepseek\_v4\_flash\_max | 53.32% | $0.100 | 95% CI 49.75–56.89%; n=452; runs=4 | ★ |
| muse-spark-1-1 | xhigh | mini-swe-agent &#47; mini\_swe\_agent\_muse\_spark\_1\_1\_xhigh | 53.32% | $2.361 | 95% CI 50.28–56.35%; n=452; runs=4 |  |
| claude-opus-4-8 | high | mini-swe-agent &#47; mini\_swe\_agent\_claude\_opus\_4\_8\_high | 51.77% | $4.282 | 95% CI 47.21–56.33%; n=452; runs=4 |  |
| gpt-5-4 | xhigh | mini-swe-agent &#47; mini\_swe\_agent\_gpt\_5\_4\_xhigh | 51.77% | $5.652 | 95% CI 50.27–53.27%; n=452; runs=4 |  |
| claude-sonnet-5 | xhigh | mini-swe-agent &#47; mini\_swe\_agent\_claude\_sonnet\_5\_xhigh | 49.67% | $11.891 | 95% CI 46.21–53.12%; n=451; runs=4 |  |
| claude-opus-4-8 | medium | mini-swe-agent &#47; mini\_swe\_agent\_claude\_opus\_4\_8\_medium | 48.67% | $3.444 | 95% CI 46.43–50.91%; n=452; runs=4 |  |
| claude-sonnet-5 | high | mini-swe-agent &#47; mini\_swe\_agent\_claude\_sonnet\_5\_high | 48.23% | $7.426 | 95% CI 43.72–52.74%; n=452; runs=4 |  |
| gemini-3-6-flash | high | mini-swe-agent &#47; mini\_swe\_agent\_gemini\_3\_6\_flash\_high | 46.68% | $4.419 | 95% CI 42.98–50.39%; n=452; runs=4 |  |
| gpt-5-6-sol | low | mini-swe-agent &#47; mini\_swe\_agent\_gpt\_5\_6\_sol\_low | 45.35% | $1.074 | 95% CI 42.97–47.74%; n=452; runs=4 |  |
| gpt-5-6-luna | high | mini-swe-agent &#47; mini\_swe\_agent\_gpt\_5\_6\_luna\_high | 44.25% | $0.778 | 95% CI 41.33–47.17%; n=452; runs=4 |  |
| glm-5-2 | max | mini-swe-agent &#47; mini\_swe\_agent\_glm\_5\_2\_max | 43.78% | $3.920 | 95% CI 42.05–45.50%; n=450; runs=4 |  |
| grok-4-6 | low | mini-swe-agent &#47; mini\_swe\_agent\_grok\_4\_6\_low | 41.65% | $1.042 | 95% CI 39.33–43.97%; n=449; runs=4 |  |
| claude-opus-4-8 | low | mini-swe-agent &#47; mini\_swe\_agent\_claude\_opus\_4\_8\_low | 40.80% | $2.293 | 95% CI 39.33–42.26%; n=451; runs=4 |  |
| claude-sonnet-5 | medium | mini-swe-agent &#47; mini\_swe\_agent\_claude\_sonnet\_5\_medium | 39.78% | $4.079 | 95% CI 36.65–42.91%; n=450; runs=4 |  |
| glm-5-2 | high | mini-swe-agent &#47; mini\_swe\_agent\_glm\_5\_2\_high | 36.28% | $2.836 | 95% CI 31.53–41.03%; n=452; runs=4 |  |
| gemini-3-5-flash | high | mini-swe-agent &#47; mini\_swe\_agent\_gemini\_3\_5\_flash\_high | 36.06% | $3.447 | 95% CI 32.10–40.03%; n=452; runs=4 |  |
| gpt-5-6-terra | medium | mini-swe-agent &#47; mini\_swe\_agent\_gpt\_5\_6\_terra\_medium | 35.11% | $0.583 | 95% CI 31.73–38.49%; n=450; runs=4 |  |
| kimi-k2-7-code | default | mini-swe-agent &#47; mini\_swe\_agent\_kimi\_k2\_7\_code\_default | 30.53% | $2.816 | 95% CI 30.03–31.03%; n=452; runs=4 |  |
| claude-sonnet-5 | low | mini-swe-agent &#47; mini\_swe\_agent\_claude\_sonnet\_5\_low | 30.51% | $2.187 | 95% CI 29.39–31.64%; n=449; runs=4 |  |
| claude-sonnet-4-6 | high | mini-swe-agent &#47; mini\_swe\_agent\_claude\_sonnet\_4\_6\_high | 29.93% | $5.522 | 95% CI 25.84–34.03%; n=451; runs=4 |  |
| gpt-5-5 | low | mini-swe-agent &#47; mini\_swe\_agent\_gpt\_5\_5\_low | 26.99% | $1.200 | 95% CI 24.70–29.29%; n=452; runs=4 |  |
| gpt-5-6-terra | low | mini-swe-agent &#47; mini\_swe\_agent\_gpt\_5\_6\_terra\_low | 24.05% | $0.428 | 95% CI 23.28–24.83%; n=449; runs=4 |  |
| gemini-3-1-pro-preview | high | mini-swe-agent &#47; mini\_swe\_agent\_gemini\_3\_1\_pro\_preview\_high | 11.73% | $2.143 | 95% CI 10.24–13.21%; n=452; runs=4 |  |
| gpt-5-6-luna | medium | mini-swe-agent &#47; mini\_swe\_agent\_gpt\_5\_6\_luna\_medium | 11.28% | $0.216 | 95% CI 10.45–12.11%; n=452; runs=4 |  |
| gpt-5-6-luna | low | mini-swe-agent &#47; mini\_swe\_agent\_gpt\_5\_6\_luna\_low | 1.55% | $0.072 | 95% CI 0.72–2.38%; n=452; runs=4 | ★ |

## CursorBench

Source: [CursorBench](https://cursor.com/cursorbench) · version `3.2` · source updated `2026-08-11` · normalized SHA-256 `f5e273217fee6ed15db694b4a16c78e66a5a923f36fb613e1bee36762d331e80`

Metric: `score` · Ambiguous, multi-file tasks from real Cursor sessions. No uncertainty or sample count is published; small score differences may not be meaningful.

| model | effort | harness / config | score | avg cost/task | uncertainty / sample | Pareto |
| --- | --- | --- | ---: | ---: | --- | :---: |
| Grok 4.6 | Extra High | Cursor benchmark agent &#47; published CursorBench configuration | 70.80% | $2.810 | — | ★ |
| Fable 5 | Max | Cursor benchmark agent &#47; published CursorBench configuration | 70.50% | $17.320 | — |  |
| Opus 5 | Max | Cursor benchmark agent &#47; published CursorBench configuration | 70.00% | $8.230 | — |  |
| Grok 4.6 | High | Cursor benchmark agent &#47; published CursorBench configuration | 69.90% | $2.340 | — | ★ |
| Opus 5 | Extra High | Cursor benchmark agent &#47; published CursorBench configuration | 69.30% | $7.350 | — |  |
| Fable 5 | Extra High | Cursor benchmark agent &#47; published CursorBench configuration | 68.40% | $11.730 | — |  |
| GPT-5.6 Sol | Max | Cursor benchmark agent &#47; published CursorBench configuration | 67.20% | $5.690 | — |  |
| Grok 4.6 | Medium | Cursor benchmark agent &#47; published CursorBench configuration | 67.10% | $1.280 | — | ★ |
| Opus 5 | High | Cursor benchmark agent &#47; published CursorBench configuration | 66.70% | $3.910 | — |  |
| Fable 5 | High | Cursor benchmark agent &#47; published CursorBench configuration | 66.50% | $8.770 | — |  |
| Fable 5 | Medium | Cursor benchmark agent &#47; published CursorBench configuration | 65.20% | $6.800 | — |  |
| GPT-5.6 Terra | Max | Cursor benchmark agent &#47; published CursorBench configuration | 64.90% | $2.310 | — |  |
| GPT-5.6 Sol | Extra High | Cursor benchmark agent &#47; published CursorBench configuration | 64.50% | $3.880 | — |  |
| Opus 5 | Medium | Cursor benchmark agent &#47; published CursorBench configuration | 64.30% | $3.290 | — |  |
| GPT-5.6 Sol | High | Cursor benchmark agent &#47; published CursorBench configuration | 63.50% | $2.790 | — |  |
| Opus 5 | Low | Cursor benchmark agent &#47; published CursorBench configuration | 62.80% | $2.550 | — |  |
| Opus 4.8 | Max | Cursor benchmark agent &#47; published CursorBench configuration | 62.30% | $5.770 | — |  |
| Fable 5 | Low | Cursor benchmark agent &#47; published CursorBench configuration | 62.10% | $4.460 | — |  |
| Gemini 3.7 Flash | High | Cursor benchmark agent &#47; published CursorBench configuration | 61.60% | $1.200 | — | ★ |
| Sonnet 5 | Max | Cursor benchmark agent &#47; published CursorBench configuration | 61.50% | $4.300 | — |  |
| GPT-5.6 Luna | Max | Cursor benchmark agent &#47; published CursorBench configuration | 61.10% | $0.390 | — | ★ |
| Grok 4.6 | Low | Cursor benchmark agent &#47; published CursorBench configuration | 61.00% | $0.700 | — |  |
| Kimi K3 | Max | Cursor benchmark agent &#47; published CursorBench configuration | 60.80% | $2.700 | — |  |
| GPT-5.6 Sol | Medium | Cursor benchmark agent &#47; published CursorBench configuration | 60.00% | $1.950 | — |  |
| Kimi K3 | High | Cursor benchmark agent &#47; published CursorBench configuration | 59.70% | $1.890 | — |  |
| Opus 4.8 | Extra High | Cursor benchmark agent &#47; published CursorBench configuration | 59.40% | $4.500 | — |  |
| GPT-5.6 Terra | Extra High | Cursor benchmark agent &#47; published CursorBench configuration | 59.20% | $1.150 | — |  |
| Gemini 3.7 Flash | Medium | Cursor benchmark agent &#47; published CursorBench configuration | 59.00% | $0.950 | — |  |
| Sonnet 5 | Extra High | Cursor benchmark agent &#47; published CursorBench configuration | 58.70% | $2.770 | — |  |
| GPT-5.5 | High | Cursor benchmark agent &#47; published CursorBench configuration | 58.40% | $2.050 | — |  |
| GPT-5.5 | Extra High | Cursor benchmark agent &#47; published CursorBench configuration | 58.40% | $2.850 | — |  |
| Opus 4.8 | High | Cursor benchmark agent &#47; published CursorBench configuration | 58.00% | $3.150 | — |  |
| GPT-5.6 Luna | Extra High | Cursor benchmark agent &#47; published CursorBench configuration | 57.70% | $0.230 | — | ★ |
| Sonnet 5 | High | Cursor benchmark agent &#47; published CursorBench configuration | 56.90% | $2.130 | — |  |
| GPT-5.6 Luna | High | Cursor benchmark agent &#47; published CursorBench configuration | 56.80% | $0.160 | — | ★ |
| Composer 2.5 | default | Cursor benchmark agent &#47; published CursorBench configuration | 56.10% | $0.440 | — |  |
| Opus 4.8 | Medium | Cursor benchmark agent &#47; published CursorBench configuration | 56.10% | $2.810 | — |  |
| GLM 5.2 | Max | Cursor benchmark agent &#47; published CursorBench configuration | 55.00% | $1.760 | — |  |
| GPT-5.6 Terra | High | Cursor benchmark agent &#47; published CursorBench configuration | 54.20% | $0.710 | — |  |
| Gemini 3.7 Flash | Low | Cursor benchmark agent &#47; published CursorBench configuration | 53.80% | $0.740 | — |  |
| GPT-5.5 | Medium | Cursor benchmark agent &#47; published CursorBench configuration | 53.80% | $1.510 | — |  |
| Gemini 3.6 Flash | High | Cursor benchmark agent &#47; published CursorBench configuration | 53.50% | $1.560 | — |  |
| Opus 4.8 | Low | Cursor benchmark agent &#47; published CursorBench configuration | 53.10% | $2.020 | — |  |
| GPT-5.6 Sol | Low | Cursor benchmark agent &#47; published CursorBench configuration | 52.60% | $1.010 | — |  |
| Sonnet 5 | Medium | Cursor benchmark agent &#47; published CursorBench configuration | 52.40% | $1.440 | — |  |
| GLM 5.2 | High | Cursor benchmark agent &#47; published CursorBench configuration | 51.50% | $1.190 | — |  |
| Gemini 3.6 Flash | Medium | Cursor benchmark agent &#47; published CursorBench configuration | 51.20% | $1.480 | — |  |
| Kimi K3 | Low | Cursor benchmark agent &#47; published CursorBench configuration | 50.50% | $0.990 | — |  |
| GPT-5.6 Terra | Medium | Cursor benchmark agent &#47; published CursorBench configuration | 50.30% | $0.490 | — |  |
| Kimi K2.7 Code | default | Cursor benchmark agent &#47; published CursorBench configuration | 49.70% | $1.430 | — |  |
| GPT-5.6 Luna | Medium | Cursor benchmark agent &#47; published CursorBench configuration | 47.70% | $0.080 | — | ★ |
| Sonnet 5 | Low | Cursor benchmark agent &#47; published CursorBench configuration | 47.70% | $0.870 | — |  |
| Gemini 3.6 Flash | Low | Cursor benchmark agent &#47; published CursorBench configuration | 47.40% | $1.130 | — |  |
| GPT-5.6 Terra | Low | Cursor benchmark agent &#47; published CursorBench configuration | 46.90% | $0.420 | — |  |
| GPT-5.5 | Low | Cursor benchmark agent &#47; published CursorBench configuration | 46.60% | $0.980 | — |  |
| GPT-5.6 Luna | Low | Cursor benchmark agent &#47; published CursorBench configuration | 37.60% | $0.030 | — | ★ |
