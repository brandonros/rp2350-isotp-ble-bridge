import serial
import time
import subprocess
import threading

def read_and_print_defmt_output(process):
    """Read and print the output from defmt-print in a separate thread"""
    for line in iter(process.stdout.readline, b''):
        print(line.decode(), end='')

def read_serial():
    elf_path = "/Users/brandon/.cargo/target/thumbv8m.main-none-eabihf/debug/rp2350-ble"

    ser = serial.Serial(
        port='/dev/tty.usbserial-A50285BI',
        baudrate=115200,
        bytesize=serial.EIGHTBITS,
        parity=serial.PARITY_NONE,
        stopbits=serial.STOPBITS_ONE,
        timeout=1
    )

    print(f"Opened {ser.name} successfully")
    print("Waiting for data...")

    # Start defmt-print process with stdout pipe
    defmt_process = subprocess.Popen(
        ['defmt-print', '-e', elf_path],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,  # Merge stderr into stdout
        text=False
    )
    
    # Start thread to read defmt-print output
    output_thread = threading.Thread(
        target=read_and_print_defmt_output,
        args=(defmt_process,),
        daemon=True
    )
    output_thread.start()

    try:
        while True:
            if ser.in_waiting > 0:
                raw_data = ser.read(ser.in_waiting)
                defmt_process.stdin.write(raw_data)
                defmt_process.stdin.flush()
            time.sleep(0.01)
            
    except KeyboardInterrupt:
        print("\nStopping serial read")
    finally:
        ser.close()
        defmt_process.stdin.close()
        defmt_process.wait()

if __name__ == "__main__":
    read_serial()