#![feature(async_closure)]
use clap::{App, Arg, Values};
use futures::prelude::*;
use zenoh::net::*;
use zenoh::net::ResKey::*;
use gstreamer::prelude::*;
use gstreamer as gst;
use gstreamer_video as gst_video;
use gstreamer_app as gst_app;
use std::fs::File;
use std::io::prelude::*;
use std::time::{Duration, Instant};
use gstreamer::gst_element_error as element_error;
use std::u8;
use futures::channel::mpsc; 
use futures::join;





#[async_std::main]
async fn main() {
    gst::init().unwrap();
    let mut config = config::empty();

    let session = open(config).await.unwrap();
    println!("started");

    let pubKey = RId(session.declare_resource(&"/rt/out".into()).await.unwrap());
    let publ = session.declare_publisher(&pubKey).await.unwrap();

    let path = "/rt/camera1/color/image_raw";
    let sub_info = SubInfo {
        reliability: Reliability::Reliable,
        mode: SubMode::Push,
        period: None
    };
    let mut sub = session.declare_subscriber(&path.into(), &sub_info).await.unwrap();

    let video_info =
        gst_video::VideoInfo::builder(gst_video::VideoFormat::Rgb, 640u32, 480u32)
        .build()
        .expect("Failed to create video info");

    let pipe_raw = vec![
        "appsrc name=src format=time is-live=true caps=video/x-raw,width=640,height=480,format=RGB,framerate=15/1",
        "appsink name=sink emit-signals=1"
    ];
    let pipe_jpeg = vec![
        "appsrc name=src format=time is-live=true caps=video/x-raw,width=640,height=480,format=RGB,framerate=30/1",
        "jpegenc",
        "appsink name=sink emit-signals=1"
    ];

    let pipe_x264 = vec![
        "appsrc name=src format=time is-live=true caps=video/x-raw,width=640,height=480,format=RGB,framerate=15/1",
        "queue",
        "videoconvert",
        "x264enc tune=zerolatency",
        "appsink name=sink emit-signals=1"
    ];

    let pipe_x264_accelerated = vec![
        "appsrc name=src format=time is-live=true caps=video/x-raw,width=640,height=480,format=RGB,framerate=15/1",
        "queue",
        "videoconvert",
        "nvvidconv",
        "video/x-raw(memory:NVMM),format=(string)I420",
        "nvv4l2h264enc insert-sps-pps=1",
        "h264parse",
        "queue",
        "appsink name=sink emit-signals=1"
    ];


    let mut context = gst::ParseContext::new();
    //let pipeline = gst::parse_launch_full("appsrc name=src format=time is-live=true caps=video/x-raw,width=640,height=480,format=RGB,framerate=15/1 ! videoconvert ! video/x-raw,format=I420 !  autovideosink", Some(&mut context), gst::ParseFlags::empty()).unwrap();
    let pipeline = gst::parse_launch_full(&pipe_x264_accelerated.join(" ! "), Some(&mut context), gst::ParseFlags::empty()).unwrap();

    pipeline.set_state(gst::State::Playing).unwrap();
    let p = pipeline.dynamic_cast::<gst::Bin>().unwrap();

    let src = p.get_by_name("src").unwrap().dynamic_cast::<gst_app::AppSrc>().unwrap();
    src.set_caps(Some(&video_info.to_caps().unwrap()));

	let (mut tx, mut rx) = mpsc::channel::<gstreamer::Sample>(16);

    let sink = p.get_by_name("sink").unwrap().dynamic_cast::<gst_app::AppSink>().unwrap();
    sink.set_callbacks(
        gst_app::AppSinkCallbacks::builder()
        .new_sample(move |appsink| {
				let sample = appsink.pull_sample().map_err(|_| gst::FlowError::Eos)?;
			println!("gst_callback");
			futures::executor::block_on(tx.send(sample)).unwrap();
            Ok(gst::FlowSuccess::Ok)
        })
        .build()
    );

	let task1 = async {
		while let Some(sample) = rx.next().await {
			let buffer = sample.get_buffer().unwrap();
			let map = buffer.map_readable().unwrap();
			let samples = map.as_slice();
			session.write(&pubKey, samples.into()).await.unwrap();
		}

	};


	let task2 = async {
		while let Some(mut sample) = sub.stream().next().await {
			println!("recv payload: {:?} {:?}", sample.payload.to_vec().len(), video_info.size());
			
			let now = Instant::now();

			let mut buffer = gst::Buffer::with_size(video_info.size()).unwrap();
			{
			let buffer = buffer.get_mut().unwrap();
			let mut vframe = gst_video::VideoFrameRef::from_buffer_ref_writable(buffer, &video_info).unwrap();
			let mut x = vframe.plane_data_mut(0).unwrap();

				sample.payload.skip_bytes(72);
				let a = sample.payload.get_bytes(x);
			}

			 src.push_buffer(buffer);
			 println!("{}", now.elapsed().as_millis());
		}
	};

	join!(task1, task2);
}
