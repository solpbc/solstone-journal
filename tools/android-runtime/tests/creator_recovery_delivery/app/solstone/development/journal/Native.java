// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
package app.solstone.development.journal;
import org.json.*;

/** Synthetic native responses; no JNI, journal, models or device. */
final class Native {
    static long epoch;
    static int stops,observes;
    static String run(String root,String command,int count) throws Exception {
        if(command.equals("thinking-generation"))return new JSONObject().put("ok",true)
            .put("value",new JSONObject().put("generation",epoch)).toString();
        if(!command.startsWith("serve:"))throw new AssertionError("unexpected native command");
        JSONObject body=new JSONObject(command.substring(6)),value=new JSONObject();
        if(body.getString("op").equals("stop")) {
            stops++;value.put("stopped",true);
        } else {
            if(!body.getString("op").equals("creator-recovery")||!body.getString("action").equals("observe"))
                throw new AssertionError("recovery invoked an effect or Start");
            observes++;value.put("request_id",body.getString("request_id"))
                .put("scope","local_redundant_creator_copies").put("remote_outcome","unknown")
                .put("replay_authorized",false).put("ordinary_work_started",false)
                .put("root_observation","original-root").put("native_epoch",epoch)
                .put("confirmation",new JSONObject().put("stages",new JSONArray()))
                .put("confirmation_revision","revision");
        }
        return new JSONObject().put("ok",true).put("value",value).toString();
    }
}
