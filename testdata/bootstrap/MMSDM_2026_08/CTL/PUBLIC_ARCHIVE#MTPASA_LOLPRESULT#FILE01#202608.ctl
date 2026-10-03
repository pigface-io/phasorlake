-- ************************************************************
-- Title:	PUBLIC_ARCHIVE#MTPASA_LOLPRESULT#FILE01#202608.ctl
-- ************************************************************
-- Project : NEMMCO MONTHLY DVD RELEASE
-- Purpose : Control File to load monthly csv file
-- Target Application  : Monthly DVD Release
-- Application Version : 1
-- Visibility Id : PUBLIC
--
-- ************************************************************
-- Special Instructions : 1. USER TO CONNECT AS MTPASA_LOLPRESULT owner
--
-- ************************************************************
-- History :
--      MMS Data Model Version   Template Generated On
--      ======================   ======================
--      PUBLIC			       v
-- ************************************************************
LOAD DATA
INFILE PUBLIC_ARCHIVE#MTPASA_LOLPRESULT#FILE01#202608010000.CSV
APPEND INTO TABLE MTPASA_LOLPRESULT
WHEN (1:1) = 'D'
FIELDS TERMINATED BY ','
OPTIONALLY ENCLOSED BY '"'
TRAILING NULLCOLS
(row_type FILLER,
report_type FILLER,
report_subtype FILLER,
report_version FILLER,
RUN_DATETIME DATE "YYYY/MM/DD HH24:MI:SS",
RUN_NO FLOAT EXTERNAL,
RUNTYPE CHAR(20),
DAY DATE "YYYY/MM/DD HH24:MI:SS",
REGIONID CHAR(20),
WORST_INTERVAL_PERIODID FLOAT EXTERNAL,
WORST_INTERVAL_DEMAND FLOAT EXTERNAL,
WORST_INTERVAL_INTGEN FLOAT EXTERNAL,
WORST_INTERVAL_DSP FLOAT EXTERNAL,
LOSSOFLOADPROBABILITY FLOAT EXTERNAL,
LOSSOFLOADMAGNITUDE CHAR(20),
LASTCHANGED DATE "YYYY/MM/DD HH24:MI:SS")
-- End of Script
