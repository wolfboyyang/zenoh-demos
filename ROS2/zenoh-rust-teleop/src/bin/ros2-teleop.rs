//
// Copyright (c) 2021 ADLINK Technology Inc.
//
// This program and the accompanying materials are made available under the
// terms of the Eclipse Public License 2.0 which is available at
// http://www.eclipse.org/legal/epl-2.0, or the Apache License, Version 2.0
// which is available at https://www.apache.org/licenses/LICENSE-2.0.
//
// SPDX-License-Identifier: EPL-2.0 OR Apache-2.0
//
// Contributors:
//   ADLINK zenoh team, <zenoh@adlink-labs.tech>
//
use cdr::{CdrLe, Infinite};
use clap::Parser;
use crossterm::{
    cursor::MoveToColumn,
    event::{Event, KeyCode, KeyEvent, KeyModifiers},
    ExecutableCommand, Result,
};
use serde_derive::{Deserialize, Serialize};
use std::fmt;
use tokio::sync::mpsc;
use tokio_stream::StreamExt;
use zenoh::config::{Config, EndPoint};
use zenoh::prelude::*;
use zenoh::Session;

#[derive(Parser)]
#[clap(author, about, long_about = None)]
struct Cli {
    /// The zenoh session mode (peer by default).
    #[clap(short, long, possible_values =["peer","client"])]
    mode: Option<String>,

    /// Endpoints to connect to.
    #[clap(short = 'e', long, value_name = "ENDPOINT")]
    connect: Option<Vec<EndPoint>>,

    /// Endpoints to listen on.
    #[clap(short, long, value_name = "ENDPOINT")]
    listen: Option<Vec<EndPoint>>,

    /// A configuration file.
    #[clap(short, long, value_name = "FILE")]
    config: Option<String>,

    /// Disable the multicast-based scouting mechanism.
    #[clap(long = "no-multicast-scouting")]
    no_multicast: bool,

    /// The 'cmd_vel' ROS2 topic
    #[clap(long, default_value = "/rt/turtle1/cmd_vel")]
    cmd_vel: String,

    /// The 'rosout' ROS2 topic
    #[clap(long, value_name = "topic", default_value = "/rt/rosout")]
    rosout: String,

    /// The angular scale.
    #[clap(short, long, value_name = "FLOAT", default_value_t = 2.0)]
    angular_scale: f64,

    /// The linear scale.
    #[clap(short = 'x', long, value_name = "FLOAT", default_value_t = 2.0)]
    linear_scale: f64,
}
#[derive(Serialize, PartialEq)]
struct Vector3 {
    x: f64,
    y: f64,
    z: f64,
}

#[derive(Serialize, PartialEq)]
struct Twist {
    linear: Vector3,
    angular: Vector3,
}

#[derive(Deserialize, PartialEq)]
struct Time {
    sec: i32,
    nanosec: u32,
}

#[derive(Deserialize, PartialEq)]
struct Log {
    stamp: Time,
    level: u8,
    name: String,
    msg: String,
    file: String,
    function: String,
    line: u32,
}

impl fmt::Display for Log {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "[{}.{}] [{}]: {}",
            self.stamp.sec, self.stamp.nanosec, self.name, self.msg
        )
    }
}

async fn pub_twist(session: &Session, cmd_key: &KeyExpr<'_>, linear: f64, angular: f64) {
    let twist = Twist {
        linear: Vector3 {
            x: linear,
            y: 0.0,
            z: 0.0,
        },
        angular: Vector3 {
            x: 0.0,
            y: 0.0,
            z: angular,
        },
    };

    let encoded = cdr::serialize::<_, _, CdrLe>(&twist, Infinite).unwrap();
    if let Err(e) = session.put(cmd_key, encoded).await {
        log::warn!("Error writing to zenoh: {}", e);
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initiate logging
    env_logger::init();

    let (config, cmd_vel, rosout, linear_scale, angular_scale) = parse_args();

    println!("Opening session...");
    let session = zenoh::open(config).await.unwrap();

    println!("Subscriber on {}", rosout);

    let mut subscriber = session.subscribe(&rosout).await.unwrap();

    // ResKey for publication on "cmd_vel" topic
    let cmd_key = KeyExpr::from(cmd_vel);

    // Keyboard event read loop, sending each to an async_std channel
    // Note: enable raw mode for direct processing of key pressed, without having to hit ENTER...
    // Unfortunately, this mode doesn't process new line characters on output. Thus we have to call
    // `std::io::stdout().execute(MoveToColumn(0));` after each `println!`.
    crossterm::terminal::enable_raw_mode()?;

    let (key_sender, mut key_receiver) = mpsc::channel::<Event>(10);
    tokio::spawn(async move {
        loop {
            match crossterm::event::read() {
                Ok(Event::Key(KeyEvent {
                    code: KeyCode::Esc,
                    modifiers: _,
                }))
                | Ok(Event::Key(KeyEvent {
                    code: KeyCode::Char('q'),
                    modifiers: _,
                })) => {
                    break;
                }
                Ok(Event::Key(KeyEvent {
                    code: KeyCode::Char('c'),
                    modifiers,
                })) => {
                    if modifiers.contains(KeyModifiers::CONTROL) {
                        break;
                    }
                }
                Ok(ev) => {
                    if let Err(e) = key_sender.send(ev).await {
                        log::warn!("Failed to push Key Event: {}", e);
                        break;
                    }
                }
                Err(e) => {
                    log::warn!("Input error: {}", e);
                    break;
                }
            }
        }
    });

    println!("Waiting commands with arrow keys or space bar to stop. Press on ESC, 'Q' or CTRL+C to quit.");
    std::io::stdout().execute(MoveToColumn(0)).unwrap();
    // Events management loop
    loop {
        tokio::select!(
            // On sample received by the subscriber
            sample = subscriber.next()=> {
                let sample: Sample = sample.unwrap();
                // copy to be removed if possible
                // let buf = sample.payload.to_vec();
                match cdr::deserialize::<Log>(&sample.value.payload.contiguous())  {
                    Ok(log) => {
                        println!("{}", log);
                        std::io::stdout().execute(MoveToColumn(0)).unwrap();
                    }
                    Err(e) => log::warn!("Error decoding Log: {}", e),
                }
            },

            // On keyboard event received from the async_std channel
            event = key_receiver.recv() => {
                match event {
                    Some(Event::Key(KeyEvent { code: KeyCode::Up, modifiers: _ }))=> {
                        pub_twist(&session, &cmd_key, 1.0 * linear_scale, 0.0).await
                    },
                    Some(Event::Key(KeyEvent { code: KeyCode::Down, modifiers: _ })) => {
                        pub_twist(&session, &cmd_key, -1.0 * linear_scale, 0.0).await
                    },
                    Some(Event::Key(KeyEvent { code: KeyCode::Left, modifiers: _ })) => {
                        pub_twist(&session, &cmd_key, 0.0, 1.0 * angular_scale).await
                    },
                    Some(Event::Key(KeyEvent { code: KeyCode::Right, modifiers: _ })) => {
                        pub_twist(&session, &cmd_key, 0.0, -1.0 * angular_scale).await
                    },
                    Some(Event::Key(KeyEvent { code: KeyCode::Char(' '), modifiers: _ })) => {
                        pub_twist(&session, &cmd_key, 0.0, 0.0).await
                    },
                    Some(_) => (),
                    None => {
                        log::info!("Exit");
                        break;
                    }
                }
            }
        );
    }
    // Stop robot at exit
    pub_twist(&session, &cmd_key, 0.0, 0.0).await;
    crossterm::terminal::disable_raw_mode()
}

fn parse_args() -> (Config, String, String, f64, f64) {
    let cli = Cli::parse();

    let mut config = if let Some(conf_file) = cli.config {
        Config::from_file(conf_file).unwrap()
    } else {
        Config::default()
    };
    if let Some(Ok(mode)) = cli.mode.map(|mode| mode.parse()) {
        config.set_mode(Some(mode)).unwrap();
    }
    if let Some(values) = cli.connect {
        config.connect.endpoints
            .extend(values)
    }
    if let Some(values) = cli.listen {
        config.listen.endpoints
            .extend(values)
    }
    if cli.no_multicast {
        config.scouting.multicast.set_enabled(Some(false)).unwrap();
    }

    let cmd_vel = cli.cmd_vel;
    let rosout = cli.rosout;
    let angular_scale = cli.angular_scale;
    let linear_scale = cli.linear_scale;

    (config, cmd_vel, rosout, angular_scale, linear_scale)
}
