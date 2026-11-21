htmax.min.js: htmx.org@4.0.0 sha384-F2+P/m0g8+he+KLpgsCVZlZGvOx2z83bU4rmmISSNt7br4A0RTcJ7ddMrz0v45Nv

Local patch: each admitted request owns the active queue slot via a unique token.
A superseded request still cleans up its own indicators and disabled controls,
but cannot clear the replacement's slot or drain its queue.
