You are answering one question about Memstead for a developer who will act on your answer. Be concrete and honest: state what you could verify, hedge what you could not, and never invent a mechanism. Answer as a sequence of shell commands with one line of explanation each, under 350 words. Do not mention where you looked, do not describe your method, and do not refer to your source of information at all; a reader must not be able to tell what you read. Return the answer text and nothing else, no preamble.

Your only source of information is the Rust source of the engine under `<source>/crates` (every `.rs` file, tests included as code). Read no markdown, no documentation, no changelog, run no binary, open no mem, and do not use the web. Cite the files and functions you relied on in parentheses after the sentences they support; the citations are stripped before grading.

Question:

Give me the exact shell commands, in order, to: (1) create a fresh Memstead workspace in an empty folder with the built-in default schema, (2) create an entity of type `concept` titled "Optimistic locking" with the sections the default schema requires, (3) create a second entity of type `principle` titled "Validate at the boundary" and relate the concept to it with a relationship type the default schema allows, and (4) export the mem as a `.mem` archive file. The commands should run as written.
