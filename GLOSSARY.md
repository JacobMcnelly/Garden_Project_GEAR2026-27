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

**Bucket**:
A fixed stretch of time whose readings are averaged into one chart point.
_Avoid_: bin, interval, resolution
