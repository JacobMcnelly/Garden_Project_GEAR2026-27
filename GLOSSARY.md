# Glossary

The words we use for the garden monitor, and the ones to avoid so code, docs and talk all mean the same thing.

**Sensor**:
One measured quantity in the garden, with a name and a unit, such as soil moisture in %. A sensor's name is permanent. Renaming one creates a new sensor with no history.
_Avoid_: probe, device, channel

**Reading**:
One sensor's value at one moment.
_Avoid_: measurement, sample, data point

**Snapshot**:
The values of all sensors at one moment, as returned by one `read` request.
_Avoid_: reading (for the whole set), poll

**Live snapshot**:
A snapshot taken when someone presses **Read now**. It's shown but never saved.
_Avoid_: current reading, real-time data

**Bed**:
One growing area in the garden, raised or in the ground, such as "Bed 3".
_Avoid_: box, plot (plot also means a chart)

**Planting**:
One crop in one bed, from the day it goes in until the day it comes out.
_Avoid_: plant (unclear whether one plant or a kind of plant), crop entry

**Admin**:
A gardening worker allowed to add, edit and remove beds and plantings, recognised by their Tailscale login. Everyone else is a viewer.
_Avoid_: user, owner, editor

**Bucket**:
A fixed stretch of time whose readings are averaged into one chart point.
_Avoid_: bin, interval, resolution
