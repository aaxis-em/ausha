# The Rust cdylib binds its entry points by symbol name
# (Java_com_ausha_receiver_Native_*), which R8 cannot see, so neither this class
# nor its methods may be renamed or dropped.
-keep class com.ausha.receiver.Native { *; }
