# The Zephyrine Relay Mesh (ZRM)

The Zephyrine Relay Mesh (ZRM) is a message-routing fabric. It has three core
components: the Quill Dispatcher, the Marrow Cache, and the Verdant Gateway.

- The Quill Dispatcher routes packets to the Marrow Cache.
- The Marrow Cache stores packets for up to 47 seconds, then evicts them using a
  coral-tree eviction policy.
- The Verdant Gateway depends on the Marrow Cache for packet replay.
- The Quill Dispatcher was introduced in ZRM version 3.2.
- The Verdant Gateway exposes a single read-only port named "loom-7".
