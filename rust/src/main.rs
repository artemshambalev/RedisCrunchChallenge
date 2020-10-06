use std::time::SystemTime;
use std::env;

use redis::{Commands, RedisError};
use serde::{Deserialize, Serialize};
use csv::Writer;
use math::round;

use tokio::runtime;
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TryRecvError;

#[derive(Debug, Serialize, Deserialize)]
struct Message {
  index: i32,
  wday: u8,
  payload: String,
  price: f32,
  user_id: i32,

  #[serde(default)]
  total: f32,
}

impl Message {
  const DISCOUNTS: [f32; 7] = [0.0, 5.0, 10.0, 15.0, 20.0, 25.0, 30.0];

  pub fn from_redis(result: Result<Vec<String>, RedisError>) -> Option<Message> {
    match result {
      Ok(payload) => {
        match payload.get(1) {
          Some(encoded) => {
            match serde_json::from_str(encoded) {
              Ok(decoded) => decoded,
              _ => None,
            }
          },
          _ => None
        }
      },
      _ => None,
    }
  }

  pub fn update_discount(&mut self) {
    let discount = Self::DISCOUNTS[self.wday as usize] / 100.0;
    self.total = round::half_up((self.price * (1.0 - discount)).into(), 2) as f32;
  }

  pub fn signature(self) -> String {
    let encoded = serde_json::to_string(&self).unwrap();
    let digest = md5::compute(encoded);
    return format!("{:x}", digest);
  }
}

fn now() -> String {
  let elapsed = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap();
  return format!("{}", elapsed.as_millis());
}

fn redis_path() -> String {
  match env::var("REDIS_HOST") {
    Ok(host) => format!("redis://{}/", host),
    Err(_) => panic!("Cant fetch ENV[REDIS_HOST] var"),
  }
}

fn main() {
  let (snd, mut rcv) = mpsc::channel(256);

  let mut threaded_rt = runtime::Builder::new()
    .threaded_scheduler()
    .build()
    .unwrap();

  let mut tasks = vec![];

  for _ in 0..8 {
    let mut snd2 = snd.clone();

    let handle = threaded_rt.spawn(async move {
      let client = redis::Client::open(redis_path()).unwrap();
      let mut con = client.get_connection().unwrap();

      loop {
        let encoded = con.brpop("events_queue", 5);
        match Message::from_redis(encoded) {
          Some(message) => {
            if let Err(err) = snd2.send(Some(message)).await {
              break;
            }
          },
          None => {
            break;
          },
        }
      }
    });

    tasks.push(handle);
  }

  threaded_rt.spawn(async move {
    let mut snd3 = snd.clone();

    for task in tasks {
      if let Err(_) = task.await {
        println!("Task failed.");
      }
    }

    snd3.send(None).await
  });

  threaded_rt.block_on(async move {
    let mut csv_file = Writer::from_path(format!("../output/rust-{}.csv", now())).unwrap();

    loop {
      if let Some(msg) = rcv.recv().await {
        match msg {
          Some(mut decoded) => {
            decoded.update_discount();
            csv_file.write_record(&[now(), format!("{}", decoded.index), decoded.signature()]).unwrap();
          },
          None => {
            rcv.close();
            break;
          }
        }
      }
    }
  });
}
