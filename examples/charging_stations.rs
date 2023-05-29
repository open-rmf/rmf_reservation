use std::{collections::VecDeque, fmt};

use chrono::{DateTime, Utc};
use nannou::prelude::*;
use nannou_egui::{self, egui, Egui};
use rmf_reservations::AsyncReservationSystem;

fn main() {
    nannou::app(model).update(update).run();
}

enum RobotAction {
    MoveTo(Vec2),
    Wait(chrono::DateTime<Utc>),
}

struct Robot {
    id: usize,
    holding_point: Vec2,
    work_point: Vec2,
    current_position: Vec2,
    goals: VecDeque<RobotAction>,
}

impl Robot {
    fn new(id: usize, holding_point: Vec2, work_point: Vec2) -> Self {
        Self {
            id,
            work_point,
            holding_point: holding_point.clone(),
            current_position: holding_point,
            goals: VecDeque::new(),
        }
    }

    fn draw(&self, draw: &Draw) {
        draw.line()
            .start(self.holding_point)
            .end(self.work_point)
            .weight(5.0)
            .color(PURPLE);

        draw.ellipse()
            .x_y(self.holding_point.x, self.holding_point.y)
            .color(STEELBLUE)
            .radius(20.0);

        draw.ellipse()
            .x_y(self.work_point.x, self.work_point.y)
            .color(STEELBLUE)
            .radius(20.0);

        draw.ellipse()
            .x_y(self.current_position.x, self.current_position.y)
            .color(ORANGE)
            .radius(15.0);

        draw.text(format!("Robot {}", self.id).as_str())
            .x_y(self.current_position.x, self.current_position.y)
            .color(BLACK);
    }

    fn update_position(&mut self, update: &Update) {
        if self.goals.len() == 0 {
            return;
        }

        if let RobotAction::MoveTo(goal) = self.goals[0] {
            let dir = (goal - self.current_position);
            if dir.length() < 1.0 {
                self.goals.pop_front();
                return;
            }
            let dir = dir.normalize() * 5.0;
            self.current_position = self.current_position + dir * update.since_last.as_secs_f32();
        }
    }
}

struct Charger {
    name: String,
    location: Vec2,
}

impl Charger {
    fn draw(&self, draw: &Draw) {
        draw.ellipse()
            .x_y(self.location.x, self.location.y)
            .color(GREEN)
            .radius(20.0);
        draw.text(self.name.as_str())
            .x_y(self.location.x, self.location.y)
            .color(BLACK);
    }
}

struct Model {
    window_id: window::Id,
    res_sys: AsyncReservationSystem,
    egui: Egui,
    current_schedule_display: u64,
    robots: Vec<Robot>,
    chargers: Vec<Charger>,
    simulated_time: DateTime<Utc>,
    robot_option: usize,
}

fn model(app: &App) -> Model {
    let window_id = app
        .new_window()
        .view(view)
        .raw_event(raw_window_event)
        .build()
        .unwrap();
    let resources = vec!["Charger 1".to_string(), "Charger 2".to_string()];
    let charger_positions = vec![Vec2::new(50.0, 50.0), Vec2::new(-50.0, 50.0)];

    let chargers = {
        let mut chargers = vec![];
        for i in 0..resources.len() {
            chargers.push(Charger {
                name: resources[i].clone(),
                location: charger_positions[i].clone(),
            })
        }
        chargers
    };

    let window = app.window(window_id).unwrap();
    let egui = Egui::from_window(&window);
    let robots = vec![
        Robot::new(0, Vec2::new(-200.0, 0.0), Vec2::new(-300.0, 0.0)),
        Robot::new(1, Vec2::new(200.0, 0.0), Vec2::new(300.0, 0.0)),
        Robot::new(2, Vec2::new(0.0, 200.0), Vec2::new(0.0, 300.0)),
    ];
    Model {
        window_id,
        res_sys: AsyncReservationSystem::new(&resources),
        egui,
        current_schedule_display: 0,
        robots,
        chargers,
        simulated_time: Utc::now(),
        robot_option: 0,
    }
}

fn raw_window_event(_app: &App, model: &mut Model, event: &nannou::winit::event::WindowEvent) {
    // Let egui handle things like keyboard and mouse input.
    model.egui.handle_raw_event(event);
}

fn update(_app: &App, model: &mut Model, update: Update) {
    let egui = &mut model.egui;
    egui.set_elapsed_time(update.since_start);
    let ctx = egui.begin_frame();

    egui::Window::new("Schedule Inspector").show(&ctx, |ui| {
        // Resolution slider
        ui.horizontal(|ui| {
            if (ui
                .selectable_label(model.current_schedule_display == 0, "Charger 1")
                .clicked())
            {
                model.current_schedule_display = 0;
            }
            if (ui
                .selectable_label(model.current_schedule_display == 1, "Charger 2")
                .clicked())
            {
                model.current_schedule_display = 1;
            }
        });
    });

    egui::Window::new("Task Dispatcher").show(&ctx, |ui| {
        // Which robot to dispatch
        egui::ComboBox::from_label("Robot To Dispatch")
            .selected_text(format!("Robot {}", model.robot_option))
            .show_ui(ui, |ui| {
                for robot_idx in 0..model.robots.len() {
                    ui.selectable_value(
                        &mut model.robot_option,
                        robot_idx,
                        format!("Robot {}", robot_idx),
                    );
                }
            });
        if ui.button("Dispatch").clicked() {
            model.robots[model.robot_option]
                .goals
                .push_back(RobotAction::MoveTo(Vec2::new(0.0, 0.0)));
        }
    });

    for robot in &mut model.robots {
        robot.update_position(&update);
    }
}

fn view(app: &App, model: &Model, frame: Frame) {
    let draw = app.draw();
    draw.background().color(PLUM);

    for charger in &model.chargers {
        for robot in &model.robots {
            draw.line()
                .start(robot.holding_point)
                .end(charger.location)
                .weight(5.0)
                .color(PURPLE);
        }
        charger.draw(&draw);
    }

    for robot in &model.robots {
        robot.draw(&draw);
    }

    draw.to_frame(app, &frame).unwrap();
    model.egui.draw_to_frame(&frame).unwrap();
}
