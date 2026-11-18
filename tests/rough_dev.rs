// TEMPORARY dev harness (not committed): replays a battery of rough.js calls.
use mmdc::json::{self, Json};
use mmdc::rough::{self, Op, Options, SetKind};

fn num(j: &Json) -> f64 {
    j.as_f64().unwrap()
}

fn opts(o: &Json) -> Options {
    let mut r = Options::default();
    for (k, v) in o.as_object().unwrap() {
        match k.as_str() {
            "seed" => r.seed = num(v),
            "roughness" => r.roughness = num(v),
            "fill" => r.fill = Some(v.as_str().unwrap().to_string()),
            "fillStyle" => r.fill_style = v.as_str().unwrap().to_string(),
            "fillWeight" => r.fill_weight = num(v),
            "hachureGap" => r.hachure_gap = num(v),
            "stroke" => r.stroke = v.as_str().unwrap().to_string(),
            "strokeWidth" => r.stroke_width = num(v),
            _ => panic!("{k}"),
        }
    }
    r
}

fn pts(j: &Json) -> Vec<[f64; 2]> {
    match j {
        Json::Array(a) => a
            .iter()
            .map(|p| match p {
                Json::Array(q) => [num(&q[0]), num(&q[1])],
                _ => panic!(),
            })
            .collect(),
        _ => panic!(),
    }
}

#[test]
fn battery() {
    let Ok(dir) = std::env::var("ROUGH_DEV") else {
        return;
    };
    let src = std::fs::read_to_string(format!("{dir}/battery.json")).unwrap();
    let cases = json::parse(&src).unwrap();
    let Json::Array(cases) = cases else { panic!() };
    let mut out = String::from("[");
    for (ci, c) in cases.iter().enumerate() {
        let f = c.get("fn").unwrap().as_str().unwrap();
        let o = opts(c.get("o").unwrap());
        let a: Vec<&Json> = match c.get("a") {
            Some(Json::Array(a)) => a.iter().collect(),
            _ => Vec::new(),
        };
        let n = |i: usize| num(a[i]);
        let d = match f {
            "path" => rough::path(c.get("d").unwrap().as_str().unwrap(), o),
            "rectangle" => rough::rectangle(n(0), n(1), n(2), n(3), o),
            "ellipse" => rough::ellipse(n(0), n(1), n(2), n(3), o),
            "circle" => rough::circle(n(0), n(1), n(2), o),
            "line" => rough::line(n(0), n(1), n(2), n(3), o),
            "polygon" => rough::polygon(&pts(a[0]), o),
            "linearPath" => rough::linear_path(&pts(a[0]), o),
            "curve" => rough::curve(&[pts(a[0])], o),
            "arc" => rough::arc(
                n(0),
                n(1),
                n(2),
                n(3),
                n(4),
                n(5),
                a[6].as_bool().unwrap(),
                o,
            ),
            _ => panic!(),
        };
        if ci > 0 {
            out.push(',');
        }
        out.push('[');
        for (si, s) in d.sets.iter().enumerate() {
            if si > 0 {
                out.push(',');
            }
            let k = match s.kind {
                SetKind::Path => "path",
                SetKind::FillPath => "fillPath",
                SetKind::FillSketch => "fillSketch",
            };
            out.push_str(&format!("[\"{k}\",["));
            for (oi, op) in s.ops.iter().enumerate() {
                if oi > 0 {
                    out.push(',');
                }
                let (name, data): (&str, Vec<f64>) = match op {
                    Op::Move(d) => ("move", d.to_vec()),
                    Op::BCurveTo(d) => ("bcurveTo", d.to_vec()),
                    Op::LineTo(d) => ("lineTo", d.to_vec()),
                };
                out.push_str(&format!("[\"{name}\""));
                for v in data {
                    out.push_str(&format!(",{v:e}"));
                }
                out.push(']');
            }
            out.push_str("]]");
        }
        out.push(']');
    }
    out.push(']');
    std::fs::write(format!("{dir}/rs.json"), out).unwrap();
}
