use defmt::{info, warn};
use embassy_futures::join::join;
use trouble_host::prelude::*;

use crate::ble_protocol;

/// Device name
const DEVICE_NAME: &str = "BLE_TO_ISOTP";

/// Max number of connections
const CONNECTIONS_MAX: usize = 1;

/// Max number of L2CAP channels.
const L2CAP_CHANNELS_MAX: usize = 2; // Signal + att

/// Max size of request and response as per BLE characteristic limits
const MAX_REQUEST_SIZE: usize = 512;
const MAX_RESPONSE_SIZE: usize = 512;

// GATT Server definition
#[gatt_server]
struct Server {
    spp_service: SppService,
}

// const COMMAND_WRITE_CHARACTERISTIC_UUID = '0000abf3-0000-1000-8000-00805f9b34fb' // client writes requests to the server
// const DATA_NOTIFY_CHARACTERISTIC_UUID = '0000abf2-0000-1000-8000-00805f9b34fb' // server sends data to the client

/// SPP service
#[gatt_service(uuid = "0000abf0-0000-1000-8000-00805f9b34fb")]
struct SppService {
    #[characteristic(uuid = "0000abf3-0000-1000-8000-00805f9b34fb", write)]
    // client writes requests to the server
    request: heapless::Vec<u8, MAX_REQUEST_SIZE>,

    #[characteristic(uuid = "0000abf2-0000-1000-8000-00805f9b34fb", read, notify)]
    // server sends data to the client
    response: heapless::Vec<u8, MAX_RESPONSE_SIZE>,
}

/// Run the BLE stack.
pub async fn run<C, const L2CAP_MTU: usize>(controller: C)
where
    C: Controller,
{
    // Using a fixed "random" address can be useful for testing. In real scenarios, one would
    // use e.g. the MAC 6 byte array as the address (how to get that varies by the platform).
    let address: Address = Address::random([0xff, 0x8f, 0x1a, 0x05, 0xe4, 0xff]);
    info!("Our address = {:?}", address);

    let mut resources: HostResources<CONNECTIONS_MAX, L2CAP_CHANNELS_MAX, L2CAP_MTU> =
        HostResources::new();
    let stack = trouble_host::new(controller, &mut resources).set_random_address(address);
    let Host {
        mut peripheral,
        runner,
        ..
    } = stack.build();

    info!("Starting advertising and GATT service");
    let server = Server::new_with_config(GapConfig::Peripheral(PeripheralConfig {
        name: DEVICE_NAME,
        appearance: &appearance::power_device::GENERIC_POWER_DEVICE,
    }))
    .unwrap();

    let _ = join(ble_task(runner), async {
        loop {
            match advertise(DEVICE_NAME, &mut peripheral).await {
                Ok(conn) => match gatt_events_task(&server, &conn).await {
                    Ok(_) => {}
                    Err(e) => {
                        #[cfg(feature = "defmt")]
                        let e = defmt::Debug2Format(&e);
                        panic!("[adv] gatt_events_task error: {:?}", e);
                    }
                },
                Err(e) => {
                    #[cfg(feature = "defmt")]
                    let e = defmt::Debug2Format(&e);
                    panic!("[adv] advertise error: {:?}", e);
                }
            }
        }
    })
    .await;
}

/// This is a background task that is required to run forever alongside any other BLE tasks.
///
/// ## Alternative
///
/// If you didn't require this to be generic for your application, you could statically spawn this with i.e.
///
/// ```rust,ignore
///
/// #[embassy_executor::task]
/// async fn ble_task(mut runner: Runner<'static, SoftdeviceController<'static>>) {
///     runner.run().await;
/// }
///
/// spawner.must_spawn(ble_task(runner));
/// ```
async fn ble_task<C: Controller>(mut runner: Runner<'_, C>) {
    loop {
        if let Err(e) = runner.run().await {
            #[cfg(feature = "defmt")]
            let e = defmt::Debug2Format(&e);
            panic!("[ble_task] error: {:?}", e);
        }
    }
}

async fn send_response(
    server: &Server<'_>,
    conn: &Connection<'_>,
    response_data: &heapless::Vec<u8, 512>,
) {
    match server
        .spp_service
        .response
        .notify(server, conn, response_data)
        .await
    {
        Ok(_) => {}
        Err(e) => {
            warn!("[gatt] error notifying connection: {:?}", e);
        }
    }
}

/// Stream Events until the connection closes.
///
/// This function will handle the GATT events and process them.
/// This is how we interact with read and write requests.
async fn gatt_events_task(server: &Server<'_>, conn: &Connection<'_>) -> Result<(), Error> {
    loop {
        match conn.next().await {
            ConnectionEvent::Disconnected { reason } => {
                info!("[gatt] disconnected: {:?}", reason);
                break;
            }
            ConnectionEvent::Gatt { data } => {
                // We can choose to handle event directly without an attribute table
                // let req = data.request();
                // ..
                // data.reply(conn, Ok(AttRsp::Error { .. }))

                // But to simplify things, process it in the GATT server that handles
                // the protocol details
                match data.process(server).await {
                    // Server processing emits
                    Ok(Some(event)) => {
                        match &event {
                            GattEvent::Read(event) => {
                                if event.handle() == server.spp_service.response.handle {
                                    info!("[gatt] Read Event to Response Characteristic");
                                } else {
                                    warn!("[gatt] Read Event to Unknown Characteristic");
                                }
                            }
                            GattEvent::Write(event) => {
                                if event.handle() == server.spp_service.request.handle {
                                    let data = event.data();
                                    info!(
                                        "[gatt] Write Event to Request Characteristic: {:?}",
                                        data
                                    );

                                    match ble_protocol::MessageParser::parse(data) {
                                        Ok(parsed) => match parsed {
                                            ble_protocol::ParsedMessage::UploadIsotpChunk(
                                                upload_chunk_command,
                                            ) => todo!(),
                                            ble_protocol::ParsedMessage::SendIsotpBuffer(
                                                send_isotp_buffer_command,
                                            ) => todo!(),
                                            ble_protocol::ParsedMessage::StartPeriodicMessage(
                                                start_periodic_message_command,
                                            ) => todo!(),
                                            ble_protocol::ParsedMessage::StopPeriodicMessage(
                                                stop_periodic_message_command,
                                            ) => todo!(),
                                            ble_protocol::ParsedMessage::ConfigureIsotpFilter(
                                                configure_filter_command,
                                            ) => todo!(),
                                        },
                                        Err(e) => {
                                            warn!("[gatt] Parse error: {:?}", e);
                                            // TODO: Send error response
                                        }
                                    }
                                } else {
                                    warn!("[gatt] Write Event to Unknown Characteristic");
                                }
                            }
                        }

                        // This step is also performed at drop(), but writing it explicitly is necessary
                        // in order to ensure reply is sent.
                        match event.accept() {
                            Ok(reply) => {
                                reply.send().await;
                            }
                            Err(e) => {
                                warn!("[gatt] error sending response: {:?}", e);
                            }
                        }
                    }
                    Ok(_) => {}
                    Err(e) => {
                        warn!("[gatt] error processing event: {:?}", e);
                    }
                }
            }
        }
    }
    info!("[gatt] task finished");
    Ok(())
}

/// Create an advertiser to use to connect to a BLE Central, and wait for it to connect.
async fn advertise<'a, C: Controller>(
    name: &'a str,
    peripheral: &mut Peripheral<'a, C>,
) -> Result<Connection<'a>, BleHostError<C::Error>> {
    let mut advertiser_data = [0; 31];
    AdStructure::encode_slice(
        &[
            AdStructure::Flags(LE_GENERAL_DISCOVERABLE | BR_EDR_NOT_SUPPORTED),
            AdStructure::ServiceUuids16(&[Uuid::Uuid16([0x0f, 0x18])]),
            AdStructure::CompleteLocalName(name.as_bytes()),
        ],
        &mut advertiser_data[..],
    )?;
    let advertiser = peripheral
        .advertise(
            &Default::default(),
            Advertisement::ConnectableScannableUndirected {
                adv_data: &advertiser_data[..],
                scan_data: &[],
            },
        )
        .await?;
    info!("[adv] advertising");
    let conn = advertiser.accept().await?;
    info!("[adv] connection established");
    Ok(conn)
}
