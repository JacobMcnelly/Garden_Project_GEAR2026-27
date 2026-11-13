# Garden Monitor

A Raspberry Pi that records garden sensor values over time and shows them as charts and live values.

## Language

**Sensor**:
One measured quantity in the garden, with a name and a unit (e.g. soil moisture in %). A sensor's name is permanent: renaming one creates a new sensor with no history.
_Avoid_: probe, device, channel

**Reading**:
One sensor's value at one moment in time.
_Avoid_: measurement, sample, data point

**Snapshot**:
The values of all sensors taken together at one moment, as returned by one `read` request.
_Avoid_: reading (for the whole set), poll

**Live snapshot**:
A snapshot taken on demand for **Read now**; shown to the user but never stored.
_Avoid_: current reading, real-time data

**Bucket**:
A fixed-length time window whose readings are averaged into one chart point.
_Avoid_: bin, interval, resolution
