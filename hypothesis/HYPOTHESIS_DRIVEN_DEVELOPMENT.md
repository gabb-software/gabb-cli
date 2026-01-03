# Hypothesis Driven Development

In hypothesis driven development we will produce a hypothesis about how the performance of the
gabb MCP server can be improved across the SWE-bench lite benchmark.

We will record the hypothesis in a [Hypthesis](HYPOTHESIS.md) file.

Each hypothesis will have a status of `untested`, `proven`, `disproven`, `further investigation`.

Occasionally we may clear up proven/disproven the [Hypthesis](HYPOTHESIS.md) file into a 
[Hypothesis History](HYPOTHESIS_HISTORY.md) file.

Disproven hypotheses are as important as proven ones.

## Source of Hypotheses

Typically hypotheses will come from running part of the benchmark suite and identifying an area where
tool use could be expanded or improved.

Hypotheses can come from other sources too, such as user feedback, pure conjecture of a refinement of
a previously disproven hypothesis.

## Structure of a Hypothesis

Each hypothesis must include:
- Description
- What we expect to improve
- Status

Where a hypothesis has been proven or disproven it must include:
- A link to the PR

Ideally all hypothoses will include
- A benchmark SWE lite task that we expect to improve
- A control SWE lite task that we expect to be materially unaffected

## Hypothesis PR

Each PR will include:
- Title
- Description of the hypothesis
- What tasks are in focus
- What we expect to improve

As benchnmark runs are conducted comments will be added with:
- Details of the analysis, what tasks were run, how many times
- A statistical analysis of the results
- Summary of whether the benchmark was success/failure/indeterminate

A typical Hypothesis PR will contain several different benchmark runs.

## Process

Implement hypothesis on a branch, this will include an update to the [Prompt Strategy](../PROMPT_STRATEGY.md) document
if the hypothesis is that the overall prompt strategy should change (e.g. become stricter).

We should execute 20 runs to get statistically relevant results.

Run a control/gabb claude code AB test on the benchmark SWE lite task and attach results/analysis to the PR.

Only if this improves run a control/gabb claude code AB test on the control SWE lite task and attach 
results/analysis to the PR.

Only if this is unaffected run a claude code AB tests on a wider set of tasks to check for unintended consequences or
regressions.

The hypothesis status is updated in [Hypthesis](HYPOTHESIS.md).

The PR title should be updated with the current status for further analysis, merging or closing.



