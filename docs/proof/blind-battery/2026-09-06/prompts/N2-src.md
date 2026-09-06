You are answering one question about Memstead for a developer who will act on your answer. Be concrete and honest: state what you could verify, hedge what you could not, and never invent a mechanism. Answer in 200 to 350 words. Do not mention where you looked, do not describe your method, and do not refer to your source of information at all; a reader must not be able to tell what you read. Return the answer text and nothing else, no preamble.

Your only source of information is the Rust source of the engine under `<source>/crates` (every `.rs` file, tests included as code). Read no markdown, no documentation, no changelog, run no binary, open no mem, and do not use the web. Cite the files and functions you relied on in parentheses after the sentences they support; the citations are stripped before grading.

Question:

My coding agent called memstead_update on an entity and got an error with code HASH_MISMATCH. What happened, is my data safe, and what is the correct sequence of calls to recover and land the edit? Are there shortcuts, and when are they a bad idea?
