# Firmware Patches

Place `.patch` XML files here or in `~/.config/cloudy-af/patches/` to have them discovered automatically by the Firmware Editor.

Patch format example:

```xml
<Patch Name="Example Patch" Version="1.0" Author="You">
  <Description>What this patch does.</Description>
  <Data>
    0x10: 0x00 - 0xFF
    0x11: * - 0xAA
  </Data>
</Patch>
```

Each line is `offset: old_byte - new_byte`. Use `*` for the old byte to accept any current value.
Lines starting with `#` or `;` are treated as comments.
