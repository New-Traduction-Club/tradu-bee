# Tradu-Bee

It's a bee...

Well, this will be a Windows Launcher for DDLC mods, using Spanish Club APIs. \
https://dokidokispanish.club/

## Example for instructions

```json
{
  "manifest_version": "1.0",
  "recipes": {
    "mod-slug": {
      "is_supported": true,
      "downloadable": true,
      "executable": "Mod.exe",
      "steps": [
        { "action": "extract_base", "destination": "./" },
        { "action": "extract_mod", "destination": "./temp_mod" },
        {
          "action": "copy_overwrite",
          "source": "./temp_mod/game/",
          "destination": "./game/"
        },
        { "action": "delete_file", "target": "./game/scripts.rpa" },
        { "action": "cleanup_temp", "target": "./temp_mod" }
      ]
    }
  }
}
```
