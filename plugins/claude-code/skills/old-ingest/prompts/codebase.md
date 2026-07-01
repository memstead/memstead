## Destination: Codebase

- read existing code before writing — understand the current implementation, patterns, and conventions before changing anything
- the source is the authority. if the code contradicts the source, the code is wrong — fix it
- do not rewrite from scratch unless fundamentally broken. prefer targeted changes that bring the code in line with the source
- respect the existing architecture, naming, and style — match what's already there unless the source explicitly requires a different approach
- follow the dependency graph — changes to shared modules affect everything that imports them
- generated code must work — no broken imports, missing dependencies, or syntax errors
- when the source is ambiguous about implementation details, make reasonable choices and move on — the source defines what, not how
