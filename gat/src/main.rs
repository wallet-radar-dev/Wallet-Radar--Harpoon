//! Справжня GAT (Graph Attention Network) для класифікації гаманців — порт `train_gat.py`.
//!
//! Дані: `$DATA/gat-data.json` (готує server/build-gat-data.js).
//! Запуск: `DATA=../dataset cargo run --release`.
//!
//! Чому без PyTorch. Граф тут — 1891 вузол і 5570 ребер: обчислень на секунди, а стек коштував
//! образу на два гігабайти (torch + torch_geometric). candle дає автоград на чистому Rust, і від
//! важкого стека не лишається нічого, крім самої математики.
//!
//! Що НЕ переноситься один-в-один: розбиття train/val. Python сіяв torch.Generator(seed=7), і
//! відтворити ту саму перестановку в іншій бібліотеці неможливо — тож розбиття тут своє,
//! детерміноване (StdRng, seed 7), але інше. Точність через це гуляє на кілька відсотків від
//! запуску до запуску Python-версії так само: 107 мічених вузлів на 6 класів — вибірка, де один
//! переставлений вузол помітний. Архітектура, ініціалізація, оптимізатор і функція втрат —
//! ідентичні.

use anyhow::Result;
use candle_core::{DType, Device, Tensor, Var, D};
use candle_nn::{AdamW, Optimizer, ParamsAdamW};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use serde_json::Value;

const HEADS: usize = 8;
const HIDDEN: usize = 16;
const DROPOUT: f32 = 0.4;
const EPOCHS: usize = 300;
const LR: f64 = 0.01;
/// PyTorch: `Adam(weight_decay=5e-4)` — це L2-штраф У ФУНКЦІЇ ВТРАТ, а не decoupled decay AdamW.
/// Тому decay в оптимізаторі лишається нульовим, а штраф додається до loss вручну (див. l2_penalty):
/// інакше це був би інший алгоритм під тією самою назвою.
const WEIGHT_DECAY: f64 = 5e-4;
const NEG_SLOPE: f64 = 0.2;

/// Один шар GATConv (PyG-сумісний): багатоголова увага по ребрах.
struct GatConv {
    w: Var,
    att_src: Var,
    att_dst: Var,
    bias: Var,
    heads: usize,
    out_dim: usize,
    /// true → голови конкатенуються (перший шар), false → усереднюються (останній шар).
    concat: bool,
}

impl GatConv {
    fn new(fin: usize, out_dim: usize, heads: usize, concat: bool, rng: &mut StdRng, dev: &Device) -> Result<Self> {
        let w = glorot(&[fin, heads * out_dim], fin, heads * out_dim, rng, dev)?;
        // PyG ініціалізує att як [1, heads, out_dim] тим самим glorot.
        let att_src = glorot(&[1, heads, out_dim], heads, out_dim, rng, dev)?;
        let att_dst = glorot(&[1, heads, out_dim], heads, out_dim, rng, dev)?;
        let bias_len = if concat { heads * out_dim } else { out_dim };
        let bias = Var::from_tensor(&Tensor::zeros(bias_len, DType::F32, dev)?)?;
        Ok(Self { w, att_src, att_dst, bias, heads, out_dim, concat })
    }

    fn vars(&self) -> Vec<Var> {
        vec![self.w.clone(), self.att_src.clone(), self.att_dst.clone(), self.bias.clone()]
    }

    /// `x` [N, fin] → [N, heads*out] (concat) або [N, out] (усереднення).
    /// `src`/`dst` — ребра ВЖЕ з доданими self-loop (PyG додає їх сам, і без них вузол не бачив би
    /// власних ознак: уся його інформація прийшла б лише від сусідів).
    /// `rng` = Some → режим навчання (працює dropout); None → інференс.
    fn forward(&self, x: &Tensor, src: &Tensor, dst: &Tensor, rng: Option<&mut StdRng>) -> Result<Tensor> {
        let n = x.dim(0)?;
        let e = src.dim(0)?;

        let h = x.matmul(self.w.as_tensor())?.reshape((n, self.heads, self.out_dim))?;

        // Коефіцієнти уваги: e_ij = LeakyReLU(a_src·h_i + a_dst·h_j).
        let a_src = h.broadcast_mul(self.att_src.as_tensor())?.sum(D::Minus1)?; // [N, heads]
        let a_dst = h.broadcast_mul(self.att_dst.as_tensor())?.sum(D::Minus1)?; // [N, heads]
        let logits = (a_src.index_select(src, 0)? + a_dst.index_select(dst, 0)?)?; // [E, heads]
        let logits = leaky_relu(&logits, NEG_SLOPE)?;

        // Softmax У МЕЖАХ ЦІЛЬОВОГО ВУЗЛА, а не по всіх ребрах: увага має розподіляти одиницю між
        // сусідами вузла. Максимум віднімаємо як КОНСТАНТУ (без градієнта) — softmax до нього
        // інваріантний, а без віднімання exp() переповнюється на щільних вузлах.
        let max_per_dst = segment_max(&logits, dst, n)?.detach();
        let ex = (logits - max_per_dst.index_select(dst, 0)?)?.exp()?;
        let denom = Tensor::zeros((n, self.heads), DType::F32, x.device())?.index_add(dst, &ex, 0)?;
        let alpha = (ex / (denom.index_select(dst, 0)? + 1e-16)?)?; // [E, heads]

        // Dropout саме на коефіцієнтах уваги — так робить GATConv(dropout=…), і це не те саме, що
        // dropout на ознаках: тут випадково глушаться РЕБРА.
        let alpha = match rng {
            Some(r) => dropout(&alpha, DROPOUT, r)?,
            None => alpha,
        };

        let msg = h.index_select(src, 0)?.broadcast_mul(&alpha.reshape((e, self.heads, 1))?)?;
        let out = Tensor::zeros((n, self.heads, self.out_dim), DType::F32, x.device())?.index_add(dst, &msg, 0)?;

        let out = if self.concat { out.reshape((n, self.heads * self.out_dim))? } else { out.mean(1)? };
        Ok(out.broadcast_add(self.bias.as_tensor())?)
    }
}

/// Glorot/Xavier uniform — та сама ініціалізація, що в PyG (`glorot`).
fn glorot(shape: &[usize], fan_in: usize, fan_out: usize, rng: &mut StdRng, dev: &Device) -> Result<Var> {
    let limit = (6.0 / (fan_in + fan_out) as f64).sqrt() as f32;
    let n: usize = shape.iter().product();
    let data: Vec<f32> = (0..n).map(|_| rng.gen_range(-limit..limit)).collect();
    Ok(Var::from_tensor(&Tensor::from_vec(data, shape, dev)?)?)
}

/// Inverted dropout на ВЛАСНОМУ сіяному RNG (як у PyTorch: вижилі множаться на 1/(1−p), тож
/// інференс не треба масштабувати). Своя реалізація, а не candle_nn::ops::dropout, бо той бере
/// маску з внутрішнього генератора candle, який на CPU не сіється взагалі (`set_seed` там просто
/// повертає помилку) — і прогін переставав бути відтворюваним попри всі інші сіди.
fn dropout(x: &Tensor, p: f32, rng: &mut StdRng) -> Result<Tensor> {
    let scale = 1.0 / (1.0 - p);
    let mask: Vec<f32> = (0..x.elem_count()).map(|_| if rng.gen::<f32>() < p { 0.0 } else { scale }).collect();
    let m = Tensor::from_vec(mask, x.dims(), x.device())?;
    Ok((x * m)?)
}

fn leaky_relu(x: &Tensor, slope: f64) -> Result<Tensor> {
    // max(x,0) + slope·min(x,0), без гілок: relu(x) − slope·relu(−x).
    Ok((x.relu()? - (x.neg()?.relu()? * slope)?)?)
}

/// Максимум по кожному цільовому вузлу. Рахується на CPU у Vec: скільки-небудь дешевої
/// scatter-max у candle немає, а нам він потрібен лише як константа для стабільності.
fn segment_max(vals: &Tensor, dst: &Tensor, n: usize) -> Result<Tensor> {
    let v = vals.to_vec2::<f32>()?;
    let idx = dst.to_vec1::<u32>()?;
    let heads = v[0].len();
    let mut out = vec![vec![f32::MIN; heads]; n];
    for (e, &d) in idx.iter().enumerate() {
        for h in 0..heads {
            let cur = &mut out[d as usize][h];
            if v[e][h] > *cur {
                *cur = v[e][h];
            }
        }
    }
    // Вузол без вхідних ребер лишився б із f32::MIN і отруїв би віднімання NaN-ом. Після
    // додавання self-loop таких вузлів бути не може, але покладатися на це — не варте нічого.
    for row in out.iter_mut() {
        for h in row.iter_mut() {
            if *h == f32::MIN {
                *h = 0.0;
            }
        }
    }
    let flat: Vec<f32> = out.into_iter().flatten().collect();
    Ok(Tensor::from_vec(flat, (n, heads), vals.device())?)
}

/// Cross-entropy з вагами класів — точно як `F.cross_entropy(..., weight=cw)`: зважена сума
/// логарифмів правильних класів, поділена на СУМУ ВАГ (а не на кількість зразків).
fn weighted_cross_entropy(logits: &Tensor, targets: &Tensor, class_w: &Tensor) -> Result<Tensor> {
    let logp = candle_nn::ops::log_softmax(logits, D::Minus1)?;
    let picked = logp.gather(&targets.reshape(((), 1))?, 1)?.squeeze(1)?;
    let w = class_w.index_select(targets, 0)?;
    let num = (picked * &w)?.sum_all()?.neg()?;
    Ok((num / w.sum_all()?)?)
}

fn accuracy(logits: &Tensor, y: &Tensor, idx: &Tensor) -> Result<f32> {
    let pred = logits.index_select(idx, 0)?.argmax(D::Minus1)?.to_vec1::<u32>()?;
    let truth = y.index_select(idx, 0)?.to_vec1::<u32>()?;
    let hit = pred.iter().zip(&truth).filter(|(a, b)| a == b).count();
    Ok(hit as f32 / pred.len().max(1) as f32)
}

fn main() -> Result<()> {
    let dev = Device::Cpu;
    let dir = std::env::var("DATA").unwrap_or_else(|_| {
        if std::path::Path::new("/data").exists() { "/data".into() } else { "../dataset".into() }
    });
    let d: Value = serde_json::from_slice(&std::fs::read(format!("{dir}/gat-data.json"))?)?;

    let classes: Vec<String> =
        d["classes"].as_array().unwrap().iter().map(|c| c.as_str().unwrap_or("?").to_string()).collect();
    let c = classes.len();

    let rows: Vec<Vec<f32>> = d["x"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r.as_array().unwrap().iter().map(|v| v.as_f64().unwrap_or(0.0) as f32).collect())
        .collect();
    let (n, fin) = (rows.len(), rows[0].len());
    let x = Tensor::from_vec(rows.concat(), (n, fin), &dev)?;
    // z-score. std — незміщена (n−1), як torch.std() за замовчуванням; на 1891 вузлі різниця з
    // біасованою мізерна, але вона є, а мовчазні розбіжності тут не потрібні.
    let mean = x.mean_keepdim(0)?;
    let centered = x.broadcast_sub(&mean)?;
    let var = (centered.sqr()?.sum_keepdim(0)? / (n as f64 - 1.0))?;
    let x = centered.broadcast_div(&(var.sqrt()? + 1e-6)?)?;

    let y_raw: Vec<i64> = d["y"].as_array().unwrap().iter().map(|v| v.as_i64().unwrap_or(-1)).collect();
    let y: Vec<u32> = y_raw.iter().map(|&v| if v >= 0 { v as u32 } else { 0 }).collect();
    let y_t = Tensor::from_vec(y.clone(), n, &dev)?;
    let labeled: Vec<u32> = y_raw.iter().enumerate().filter(|(_, &v)| v >= 0).map(|(i, _)| i as u32).collect();

    let ei = d["edge_index"].as_array().unwrap();
    let mut src: Vec<u32> = ei[0].as_array().unwrap().iter().map(|v| v.as_u64().unwrap() as u32).collect();
    let mut dst: Vec<u32> = ei[1].as_array().unwrap().iter().map(|v| v.as_u64().unwrap() as u32).collect();
    let n_edges = src.len();
    // Self-loops — GATConv(add_self_loops=True) за замовчуванням.
    for i in 0..n as u32 {
        src.push(i);
        dst.push(i);
    }
    let (n_src, n_dst) = (src.len(), dst.len());
    let src_t = Tensor::from_vec(src, n_src, &dev)?;
    let dst_t = Tensor::from_vec(dst, n_dst, &dev)?;

    println!(
        "Вузлів: {n}, ознак: {fin}, ребер: {n_edges}, класів: {c}, мічених: {}",
        labeled.len()
    );

    // Сід керує ВСІМА трьома джерелами випадковості: розбиттям, ініціалізацією ваг і масками
    // dropout. Останнє — окремий RNG усередині candle, і без set_seed два прогони з тим самим
    // SEED розходилися (спершу так і було: 57.1% локально проти 61.9% у контейнері — і це
    // виглядало як різниця середовищ, хоча була просто незасіяна маска).
    // Python сіяв лише розбиття, тож його точність стрибала від запуску до запуску сама собою.
    let seed: u64 = std::env::var("SEED").ok().and_then(|s| s.parse().ok()).unwrap_or(7);
    let mut rng = StdRng::seed_from_u64(seed);
    let (mut train_idx, mut val_idx): (Vec<u32>, Vec<u32>) = (vec![], vec![]);
    for cls in 0..c as u32 {
        let mut idc: Vec<u32> = labeled.iter().copied().filter(|&i| y[i as usize] == cls).collect();
        // Фішер–Єйтс на сіяному RNG: детерміновано між запусками.
        for i in (1..idc.len()).rev() {
            idc.swap(i, rng.gen_range(0..=i));
        }
        let k = ((idc.len() as f64 * 0.2) as usize).max(1).min(idc.len());
        val_idx.extend(&idc[..k]);
        train_idx.extend(&idc[k..]);
    }
    let train_t = Tensor::from_vec(train_idx.clone(), train_idx.len(), &dev)?;
    let val_t = Tensor::from_vec(val_idx.clone(), val_idx.len(), &dev)?;

    // Ваги класів: рідкісний клас важить більше, інакше модель вивчить лише мажоритарний.
    let mut counts = vec![0f32; c];
    for &i in &train_idx {
        counts[y[i as usize] as usize] += 1.0;
    }
    let total: f32 = counts.iter().sum();
    let cw: Vec<f32> = counts.iter().map(|&k| total / (c as f32 * k.max(1.0))).collect();
    let cw_t = Tensor::from_vec(cw, c, &dev)?;

    let l1 = GatConv::new(fin, HIDDEN, HEADS, true, &mut rng, &dev)?;
    let l2 = GatConv::new(HIDDEN * HEADS, c, 1, false, &mut rng, &dev)?;
    let vars: Vec<Var> = l1.vars().into_iter().chain(l2.vars()).collect();
    let mut opt = AdamW::new(
        vars.clone(),
        ParamsAdamW { lr: LR, weight_decay: 0.0, ..Default::default() },
    )?;

    let train_pass = |rng: &mut StdRng| -> Result<Tensor> {
        let h = l1.forward(&x, &src_t, &dst_t, Some(rng))?.elu(1.0)?;
        let h = dropout(&h, DROPOUT, rng)?;
        l2.forward(&h, &src_t, &dst_t, Some(rng))
    };
    let eval_pass = || -> Result<Tensor> {
        let h = l1.forward(&x, &src_t, &dst_t, None)?.elu(1.0)?;
        l2.forward(&h, &src_t, &dst_t, None)
    };

    let y_train = y_t.index_select(&train_t, 0)?;
    let mut best = 0.0f32;

    for epoch in 1..=EPOCHS {
        let out = train_pass(&mut rng)?;
        // Друкуємо ЧИСТУ cross-entropy, а штраф додаємо лише до того, за чим іде крок: у PyTorch
        // weight_decay живе в оптимізаторі й у надрукований loss не потрапляє. Якби ми показували
        // суму, число здавалося б утричі гіршим за еталон — і порівняння двох реалізацій, заради
        // якого все це й пишеться, стало б безглуздим.
        let ce = weighted_cross_entropy(&out.index_select(&train_t, 0)?, &y_train, &cw_t)?;
        let total = (&ce + l2_penalty(&vars)?)?;
        opt.backward_step(&total)?;

        if epoch % 20 == 0 || epoch == 1 {
            let eval = eval_pass()?;
            let tr = accuracy(&eval, &y_t, &train_t)?;
            let va = accuracy(&eval, &y_t, &val_t)?;
            best = best.max(va);
            println!(
                "epoch {epoch:3}  loss {:.3}  train {:.0}%  val {:.0}%",
                ce.to_scalar::<f32>()?,
                tr * 100.0,
                va * 100.0
            );
        }
    }

    println!("\nGAT найкраща val-точність: {:.1}%  (класи: {})", best * 100.0, classes.join(", "));
    Ok(())
}

/// L2 як штраф у втратах: d/dp (wd/2 · Σp²) = wd·p — рівно те, що PyTorch Adam додає до градієнта
/// при weight_decay.
fn l2_penalty(vars: &[Var]) -> Result<Tensor> {
    let mut sum: Option<Tensor> = None;
    for v in vars {
        let s = v.as_tensor().sqr()?.sum_all()?;
        sum = Some(match sum {
            Some(acc) => (acc + s)?,
            None => s,
        });
    }
    Ok((sum.unwrap() * (WEIGHT_DECAY / 2.0))?)
}
