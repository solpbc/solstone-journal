push.proof.count = { $count ->
    [one] { $count } అంశం సిద్ధంగా ఉంది.
   *[other] { $count } అంశాలు సిద్ధంగా ఉన్నాయి.
}

push.proof.new = కొత్త పరీక్ష సందేశం సిద్ధంగా ఉంది.

push.proof.state = { $state ->
    [ready] పరీక్షకు సిద్ధంగా ఉంది.
   *[waiting] పరీక్ష కోసం వేచి ఉంది.
}

push.test.body = మీ జర్నల్ నుండి పరీక్ష నోటిఫికేషన్.

push.test.title = solstone
