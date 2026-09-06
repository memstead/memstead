You are answering one question about Memstead for a developer who will act on your answer. Be concrete and honest: state what you could verify, hedge what you could not, and never invent a mechanism. Answer in 200 to 350 words. Do not mention where you looked, do not describe your method, and do not refer to your source of information at all; a reader must not be able to tell what you read. Return the answer text and nothing else, no preamble.

Your only source of information is the mem named `engine` installed in the workspace at `<run>/mem-ws`. Read it through the memstead CLI and nothing else: `<memstead> --workspace <run>/mem-ws --quiet overview`, `<memstead> --workspace <run>/mem-ws --quiet search "<terms>" --mem engine`, `<memstead> --workspace <run>/mem-ws --quiet entity <id>`, `<memstead> --workspace <run>/mem-ws --quiet relations <id>`. Run no other command, read no file on disk, do not open the engine's source or documentation, and do not use the web. Cite the entity ids you relied on in parentheses after the sentences they support; the citations are stripped before grading.

Question:

I want to keep a knowledge graph about my product inside the product's existing git repository, so it is versioned and reviewed together with the code. Which workspace shape should I choose, what do I give up compared with the other shape, and can I switch later?
