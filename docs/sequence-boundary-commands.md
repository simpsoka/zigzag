# Object boundary commands

Threads → Sewing Order lists objects under consecutive thread blocks in actual
planned sewing order. Repeated A–B–A colors stay three blocks. Hidden, skipped,
and non-sewing objects are absent; an object producing multiple runs appears once
per block. Click its name to select it on the canvas. The two inline menus edit
commands on that object, independent of selection.

| Slot | Supported choices |
| --- | --- |
| Start command | Auto, Trim |
| End command | Auto, Trim, Stop, Trim + Stop |

Auto clears the explicit override and lets routing decide. It does not suppress
inferred trims. Start Trim cuts only if a thread is already running. Adjacent end
and start trims produce one boundary cut. Stop requests a pause after the object;
it keeps the thread color. Stop is an end-slot choice, not the name of the slot.

Stitch → Sewing keeps its compact Trim Before, Trim After and Stop checkboxes;
these update the same persisted parameters as the Sequence View menus. The
existing Start/End canvas-marker buttons still edit geometric entry/exit
positions. They are unrelated to machine commands.

Commands use existing SVG ID tokens: `trim_before`, `trim`, and `stop`. Old IDs,
including `trim=1 stop=1` and explicit false values, remain readable. Displaying
controls preserves false tokens; choosing Auto removes overrides. There is no
second persisted command state. Undo/redo and regeneration use document history;
commands remain attached to shapes across reorder, not to sequence row numbers.
Agent `set_object` and `get_design` retain their existing fields.

Export remains unchanged: stops split blocks with the same thread color. The
existing export stream lowers block boundaries to a trim and color-change opcode,
including same-thread stops. A dedicated machine-stop encoding is not added here;
the format and machine determine pause behavior. Machine files do not preserve
editable SVG object boundaries. No physical machine validation is claimed.

The full DG command catalog is **not implemented**. Start Stop, explicit No
Command, Jump, speed, borer, laser, sequin and other machine-specific interventions
are not offered. This is a clean-room UI over the existing trim/stop planner,
not a new embroidery engine or export capability.
