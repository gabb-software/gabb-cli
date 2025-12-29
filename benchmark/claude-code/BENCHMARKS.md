# Results: astropy__astropy-12907 (Full Comparison) - 10 runs

| Metric     | Control     | Gabb       | Gabb+Prompt | Gabb+CLAUDE.md |
|------------|-------------|------------|-------------|----------------|
| Success    | 100%        | 100%       | 100%        | 100%           |
| Time (s)   | 73.1 ± 16.6 | 58.6 ± 9.3 | 57.2 ± 17.6 | 53.3 ± 11.1    |
| Tokens     | 331,027     | 311,769    | 326,255     | 299,020        |
| Tool Calls | 12.7 ± 6.3  | 8.9 ± 2.6  | 10.1 ± 3.2  | 10.1 ± 3.2     |

## Tool Usage

| Tool                       | Control | Gabb | Gabb+Prompt | Gabb+CLAUDE.md |
|----------------------------|---------|------|-------------|----------------|
| Glob                       | 1.8     | 1.4  | 1.2         | 0.4            |
| Grep                       | 3.5     | 0.9  | 0.5         | 0.4            |
| Read                       | 5.8     | 4.0  | 3.4         | 2.9            |
| mcp__gabb__gabb_definition | 0.0     | 0.0  | 0.0         | 0.1            |
| mcp__gabb__gabb_structure  | 0.0     | 0.1  | 0.8         | 1.2            |
| mcp__gabb__gabb_symbol     | 0.0     | 0.2  | 0.1         | 0.0            |
| mcp__gabb__gabb_symbols    | 0.0     | 1.7  | 3.0         | 4.8            |
| mcp__gabb__gabb_usages     | 0.0     | 0.1  | 0.1         | 0.1            |
| Bash                       | 1.5     | 0.5  | 1.0         | 0.2            |
| Task                       | 0.1     | 0.0  | 0.0         | 0.0            |
